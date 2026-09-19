//! Automatic path repair between machines.
//!
//! A packed file at `/Users/Alice/.hermes/config.yaml` that references
//! `/Users/Alice/.hermes` inside its contents (or whose manifest stores
//! absolute paths) must become `/Users/Bob/.hermes` on the target.
//!
//! The mapper records an ordered list of (source prefix, target prefix)
//! replacements. Longer prefixes first so `/Users/Alice/.hermes` wins over
//! `/Users/Alice`.

use std::path::{Path, PathBuf};

/// A single prefix replacement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PathRule {
    /// Source-side prefix, always with `/` separators, no trailing slash.
    pub from: String,
    /// Target-side prefix.
    pub to: String,
}

impl PathRule {
    /// Build a rule from two native paths, normalized to `/` form.
    pub fn new(from: &Path, to: &Path) -> Self {
        Self {
            from: from.to_string_lossy().replace('\\', "/"),
            to: to.to_string_lossy().replace('\\', "/"),
        }
    }
}

/// An ordered set of prefix replacements, longest first.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PathMapper {
    pub source_home: String,
    pub source_hermes_home: String,
    pub source_workspace: String,
    pub target_home: String,
    pub target_hermes_home: String,
    pub target_workspace: String,
}

impl PathMapper {
    pub fn new(
        source_home: &str,
        source_hermes_home: &str,
        source_workspace: &str,
        target_home: &str,
        target_hermes_home: &str,
        target_workspace: &str,
    ) -> Self {
        // Normalize any Windows backslashes to `/` so cross-platform
        // replacement works uniformly (rule matching is `/`-based).
        let n = |s: &str| -> String { s.replace('\\', "/") };
        Self {
            source_home: n(source_home),
            source_hermes_home: n(source_hermes_home),
            source_workspace: n(source_workspace),
            target_home: n(target_home),
            target_hermes_home: n(target_hermes_home),
            target_workspace: n(target_workspace),
        }
    }

    /// Ordered replacement pairs. Hermes-home first (most specific), then
    /// workspace, then home. Skips no-ops.
    pub fn rules(&self) -> Vec<PathRule> {
        let mut pairs: Vec<(String, String)> = vec![
            (
                self.source_hermes_home.clone(),
                self.target_hermes_home.clone(),
            ),
            (self.source_workspace.clone(), self.target_workspace.clone()),
            (self.source_home.clone(), self.target_home.clone()),
        ];
        pairs.retain(|(a, b)| !a.is_empty() && !b.is_empty() && a != b);
        pairs.sort_by_key(|(a, _)| std::cmp::Reverse(a.len()));
        pairs
            .into_iter()
            .map(|(from, to)| PathRule { from, to })
            .collect()
    }

    /// Rewrite one path string.
    pub fn map_path(&self, p: &str) -> String {
        Self::map_with(&self.rules(), p)
    }

    /// Rewrite one path using a caller-provided rule set (from a manifest).
    pub fn map_with(rules: &[PathRule], p: &str) -> String {
        let mut out = p.replace('\\', "/");
        for r in rules {
            if out == r.from {
                out = r.to.clone();
                break;
            }
            if let Some(rest) = out.strip_prefix(&r.from) {
                if rest.is_empty() || rest.starts_with('/') {
                    out = format!("{}{}", r.to, rest);
                    break;
                }
            }
        }
        out
    }

    /// Rewrite a whole native path.
    pub fn map_native(&self, p: &Path) -> PathBuf {
        let s = p.to_string_lossy().to_string();
        let out = self.map_path(&s);
        if cfg!(windows) {
            // Turn `/` back to `\` on Windows.
            PathBuf::from(out.replace('/', "\\"))
        } else {
            PathBuf::from(out)
        }
    }

    /// Build the rules for a target machine that is being restored onto.
    /// `package` holds source-side prefixes; the target side is filled from
    /// the live machine.
    pub fn for_restore(
        source_home: &str,
        source_hermes_home: &str,
        source_workspace: &str,
    ) -> Self {
        let th = crate::platform::current().home_dir();
        let tt = crate::platform::current()
            .hermes_home()
            .unwrap_or_else(|| std::path::PathBuf::from(format!("{}/.hermes", th.display())));
        let tw = crate::platform::current().workspace_dir();
        Self::new(
            source_home,
            source_hermes_home,
            source_workspace,
            &th.to_string_lossy(),
            &tt.to_string_lossy(),
            &tw.to_string_lossy(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac_rules() -> Vec<PathRule> {
        PathMapper::new(
            "/Users/Alice",
            "/Users/Alice/.hermes",
            "/Users/Alice/.hermes/workspace",
            "/Users/Bob",
            "/Users/Bob/.hermes",
            "/Users/Bob/.hermes/workspace",
        )
        .rules()
    }

    #[test]
    fn hermes_home_wins_over_home() {
        let rules = mac_rules();
        assert_eq!(
            PathMapper::map_with(&rules, "/Users/Alice/.hermes/config.yaml"),
            "/Users/Bob/.hermes/config.yaml"
        );
    }

    #[test]
    fn plain_home_path_maps_to_home() {
        let rules = mac_rules();
        assert_eq!(
            PathMapper::map_with(&rules, "/Users/Alice/Documents/notes.md"),
            "/Users/Bob/Documents/notes.md"
        );
    }

    #[test]
    fn prefix_boundaries_do_not_bleed() {
        let rules = mac_rules();
        // "/Users/Alicex" must NOT match the "/Users/Alice" rule.
        assert_eq!(
            PathMapper::map_with(&rules, "/Users/Alicex/file"),
            "/Users/Alicex/file"
        );
    }

    #[test]
    fn windows_backslashes_normalize() {
        let m = PathMapper::new(
            "C:\\Users\\Alice",
            "C:\\Users\\Alice\\.hermes",
            "",
            "C:\\Users\\Bob",
            "C:\\Users\\Bob\\.hermes",
            "",
        );
        assert_eq!(
            PathMapper::map_with(&m.rules(), "C:\\Users\\Alice\\.hermes\\skills"),
            "C:/Users/Bob/.hermes/skills"
        );
    }

    #[test]
    fn identical_paths_are_noop_rules() {
        let m = PathMapper::new("/a", "/a/h", "/a/h/w", "/a", "/a/h", "/a/h/w");
        assert!(m.rules().is_empty());
    }
}
