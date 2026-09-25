//! Secrets handling.
//!
//! API keys, tokens, and credentials are never written in plaintext to the
//! manifest or logs. At pack time they are collected, encrypted with
//! AES-256-GCM under a key derived (Argon2id) from a user passphrase, and
//! stored as a single `secrets.enc` blob inside the package. At restore time
//! the user is asked for the passphrase; the stored KDF digest verifies it,
//! and the raw Argon2 output becomes the AES key.
//!
//! Credentials that cannot be safely migrated are surfaced as
//! "requires re-authentication" — never a fake success.

use aes_gcm::aead::{AeadInPlace, Tag};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::Argon2;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;

/// Argon2 KDF params + salt + raw 32-byte digest, stored compactly.
/// The digest (Argon2id output over passphrase || salt) IS the AES-256 key;
/// on verify we recompute and constant-time compare.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KdfMeta {
    /// 16-byte salt (hex).
    pub salt: String,
    /// 32-byte argon2 output (hex).
    pub digest: String,
}

/// An encrypted secrets container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsBundle {
    /// KDF material (salt + stored digest) for the passphrase.
    pub kdf: KdfMeta,
    /// entry name -> base64([12-byte nonce | AES-GCM ciphertext+tag]).
    pub entries: BTreeMap<String, String>,
}

/// Minimal base64 (standard alphabet, with padding). Keeps the dep tree lean.
pub mod b64 {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                A[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                A[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        let chars: Vec<char> = s.chars().filter(|c| *c != '\n').collect();
        let mut out = Vec::with_capacity(chars.len() / 4 * 3);
        let mut buf: u32 = 0;
        let mut bits = 0;
        for c in chars.iter() {
            if *c == '=' {
                continue;
            }
            let v = A
                .iter()
                .position(|&b| b as char == *c)
                .ok_or_else(|| format!("bad b64 char {:?}", c))? as u32;
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        Ok(out)
    }
}

/// Hex encode.
fn hex_enc(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// Hex decode (pairs of chars -> bytes).
fn hex_dec(s: &str) -> Option<Vec<u8>> {
    fn nib(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.bytes()
        .map(nib)
        .collect::<Option<Vec<u8>>>()
        .map(|nibs| nibs.chunks(2).map(|c| (c[0] << 4) | c[1]).collect())
}

/// Fresh KDF material for a passphrase (random 16-byte salt).
/// Returns (metadata, the raw 32-byte AES key).
fn new_kdf(passphrase: &str) -> Result<(KdfMeta, [u8; 32]), crate::MigratorError> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let mut out = [0u8; 32];
    // Argon2id (t=2, m=19 MiB, p=1) — the default profile; solid for an
    // interactive passphrase.
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| crate::MigratorError::Secrets(e.to_string()))?;
    Ok((
        KdfMeta {
            salt: hex_enc(&salt),
            digest: hex_enc(&out),
        },
        out,
    ))
}

/// Verify a passphrase against stored KDF material; returns the AES key.
fn key_for_passphrase(passphrase: &str, meta: &KdfMeta) -> Result<[u8; 32], crate::MigratorError> {
    let salt = hex_dec(&meta.salt)
        .ok_or_else(|| crate::MigratorError::Secrets("corrupt kdf salt".into()))?;
    if salt.len() != 16 {
        return Err(crate::MigratorError::Secrets("bad kdf salt length".into()));
    }
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| crate::MigratorError::Secrets(e.to_string()))?;
    let want = hex_dec(&meta.digest)
        .ok_or_else(|| crate::MigratorError::Secrets("corrupt kdf digest".into()))?;
    if want.len() != 32 {
        return Err(crate::MigratorError::Secrets(
            "bad kdf digest length".into(),
        ));
    }
    // constant-time compare
    let mut diff = 0u8;
    for (a, b) in out.iter().zip(want.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        return Err(crate::MigratorError::Secrets(
            "passphrase does not match the migration package".into(),
        ));
    }
    Ok(out)
}

/// Encrypt a set of (name -> (source, plaintext_bytes)) into a bundle.
pub fn encrypt_secrets(
    passphrase: &str,
    items: BTreeMap<String, (String, Vec<u8>)>,
) -> Result<SecretsBundle, crate::MigratorError> {
    let (meta, key) = new_kdf(passphrase)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| crate::MigratorError::Secrets(format!("aes init: {}", e)))?;
    let mut entries = BTreeMap::new();
    for (name, (_source, plain)) in items {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut ct = plain;
        let tag: Tag<Aes256Gcm> = cipher
            .encrypt_in_place_detached(nonce, &[], &mut ct)
            .map_err(|_| crate::MigratorError::Secrets("encrypt failed".into()))?;
        let mut blob = Vec::with_capacity(12 + ct.len() + 16);
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        blob.extend_from_slice(tag.as_slice());
        entries.insert(name, b64::encode(&blob));
    }
    Ok(SecretsBundle { kdf: meta, entries })
}

/// Decrypt a bundle back into (name -> plaintext).
pub fn decrypt_secrets(
    passphrase: &str,
    bundle: &SecretsBundle,
) -> Result<BTreeMap<String, Vec<u8>>, crate::MigratorError> {
    let key = key_for_passphrase(passphrase, &bundle.kdf)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| crate::MigratorError::Secrets(format!("aes init: {}", e)))?;
    let mut out = BTreeMap::new();
    for (name, b64ct) in &bundle.entries {
        // layout: nonce(12) | ciphertext(n-28) | tag(16)
        let blob = b64::decode(b64ct).map_err(crate::MigratorError::Secrets)?;
        if blob.len() < 12 + 16 {
            return Err(crate::MigratorError::Secrets("blob too short".into()));
        }
        let nonce = Nonce::from_slice(&blob[..12]);
        let tag_start = blob.len() - 16;
        let tag = Tag::<Aes256Gcm>::clone_from_slice(&blob[tag_start..]);
        let mut buf = blob[12..tag_start].to_vec();
        cipher
            .decrypt_in_place_detached(nonce, &[], &mut buf, &tag)
            .map_err(|_| {
                crate::MigratorError::Secrets("decryption failed (wrong passphrase?)".into())
            })?;
        out.insert(name.clone(), buf);
    }
    Ok(out)
}

/// Names of the well-known secret files under a hermes home.
pub const SECRET_FILES: &[&str] = &["auth.json", ".env", "channel_directory.json"];

/// SHA-256 hex of a secret file (used to prove it round-tripped).
pub fn content_hash_hex(data: &[u8]) -> String {
    let mut h = [0u8; 32];
    h.copy_from_slice(&sha2::Sha256::digest(data));
    h.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let raw = b"AGNES_API_KEY=secret-123";
        assert_eq!(b64::decode(&b64::encode(raw)).unwrap(), raw);
        // Padding cases
        assert_eq!(b64::decode(&b64::encode(b"a")).unwrap(), b"a");
        assert_eq!(b64::decode(&b64::encode(b"ab")).unwrap(), b"ab");
        assert!(b64::decode("!!!").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(
            hex_dec(&hex_enc(&[0xde, 0xad, 0xbe, 0xef])).unwrap(),
            [0xde, 0xad, 0xbe, 0xef]
        );
        assert!(hex_dec("zz").is_none());
    }

    #[test]
    fn secrets_encrypt_decrypt_roundtrip() {
        let items: BTreeMap<String, (String, Vec<u8>)> = BTreeMap::from([
            (
                "auth.json".to_string(),
                ("auth.json".to_string(), b"{\"token\":\"abc\"}".to_vec()),
            ),
            (
                ".env".to_string(),
                (".env".to_string(), b"AGNES_API_KEY=xyz".to_vec()),
            ),
        ]);
        let bundle = encrypt_secrets("correct horse", items).unwrap();
        let got = decrypt_secrets("correct horse", &bundle).unwrap();
        assert_eq!(got["auth.json"], b"{\"token\":\"abc\"}");
        assert_eq!(got[".env"], b"AGNES_API_KEY=xyz");
    }

    #[test]
    fn wrong_passphrase_fails() {
        let items: BTreeMap<String, (String, Vec<u8>)> =
            BTreeMap::from([("x".to_string(), ("x".to_string(), b"data".to_vec()))]);
        let bundle = encrypt_secrets("right", items).unwrap();
        assert!(decrypt_secrets("wrong", &bundle).is_err());
    }

    #[test]
    fn content_hash_is_sha256() {
        assert_eq!(
            content_hash_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
