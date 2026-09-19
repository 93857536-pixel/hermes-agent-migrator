//! SHA-256 checksums for integrity verification.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{self, Read};

/// Hex-encoded SHA-256 of a file.
pub fn file_sha256(path: &std::path::Path) -> Result<String, io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Hex-encoded SHA-256 of a byte slice.
pub fn bytes_sha256(data: &[u8]) -> String {
    to_hex(&Sha256::digest(data))
}

fn to_hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Compute checksums for every regular file under `root` (recursively).
/// Keys are forward-slash relative paths.
pub fn checksum_tree(root: &std::path::Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut BTreeMap<String, String>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for e in rd {
        let e = match e {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = e.path();
        let md = match e.metadata() {
            Ok(md) => md,
            Err(_) => continue,
        };
        if md.is_dir() {
            walk(base, &p, out);
        } else if md.is_file() {
            if let Ok(hex) = file_sha256(&p) {
                let rel = p
                    .strip_prefix(base)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                out.insert(rel, hex);
            }
        }
    }
}

/// Verify `expected` (path -> sha256) against the tree at `root`.
/// Returns the list of failing paths (empty = all good).
pub fn verify_tree(root: &std::path::Path, expected: &BTreeMap<String, String>) -> Vec<String> {
    let mut missing = Vec::new();
    for (rel, want) in expected {
        let p = root.join(rel);
        match file_sha256(&p) {
            Ok(got) => {
                if got != *want {
                    missing.push(rel.clone());
                }
            }
            Err(_) => missing.push(rel.clone()),
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_empty_is_known_vector() {
        assert_eq!(
            bytes_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn tree_roundtrip_and_detects_tamper() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::write(tmp.path().join("x.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("a/b/y.bin"), [0u8, 1, 2]).unwrap();
        let map = checksum_tree(tmp.path());
        assert_eq!(map.len(), 2);
        assert!(verify_tree(tmp.path(), &map).is_empty());

        std::fs::write(tmp.path().join("x.txt"), b"tampered").unwrap();
        let bad = verify_tree(tmp.path(), &map);
        assert_eq!(bad, vec!["x.txt"]);
    }
}
