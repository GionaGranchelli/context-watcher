//! Deterministic workspace symbol indexing (Milestone A).
//!
//! `SymbolIndex` maps stable `repo://<rel_path>` / `repo://<rel_path>#<symbol>` URIs to
//! exact byte and line ranges. All iteration is over a `BTreeMap`, so rendering,
//! resolution fallbacks, and build output are fully deterministic regardless of
//! filesystem enumeration order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable URI for a workspace symbol: `repo://<rel_path>` or `repo://<rel_path>#<symbol>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolUri(pub String);

/// Language-agnostic classification of an indexed declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    File,
    Module,
    Struct,
    Enum,
    Trait,
    Impl,
    Function,
    Class,
}

/// One indexed declaration with an exact byte/line range into its source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub uri: SymbolUri,
    pub rel_path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: SymbolKind,
    pub signature: String,
}

/// Deterministic workspace symbol index keyed by URI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SymbolIndex {
    pub entries: BTreeMap<SymbolUri, SymbolEntry>,
}

const MAX_RESOLVE_SUGGESTIONS: usize = 5;

impl SymbolIndex {
    /// Resolve a `repo://...` URI to its entry.
    ///
    /// On a miss the error carries up to [`MAX_RESOLVE_SUGGESTIONS`] nearest valid URIs:
    /// entries sharing the requested file prefix when any exist, otherwise overall
    /// closest. Ranking is deterministic: common-prefix length descending, then
    /// lexicographic ascending.
    pub fn resolve(&self, uri: &str) -> Result<&SymbolEntry, Vec<String>> {
        let key = SymbolUri(uri.to_owned());
        if let Some(entry) = self.entries.get(&key) {
            return Ok(entry);
        }

        let file_prefix = uri.split_once('#').map(|(file, _)| file.to_string());
        let candidates: Vec<&SymbolUri> = if let Some(prefix) = &file_prefix
            && self
                .entries
                .keys()
                .any(|k| file_part(&k.0) == prefix.as_str())
        {
            self.entries
                .keys()
                .filter(|k| file_part(&k.0) == prefix.as_str())
                .collect()
        } else {
            self.entries.keys().collect()
        };

        let mut scored: Vec<(usize, &SymbolUri)> = candidates
            .into_iter()
            .map(|candidate| (common_prefix_len(candidate.0.as_str(), uri), candidate))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.0.cmp(&b.1.0)));

        Err(scored
            .into_iter()
            .take(MAX_RESOLVE_SUGGESTIONS)
            .map(|(_, candidate)| candidate.0.clone())
            .collect())
    }
}

/// The file part of a `repo://...` URI (everything before the first `#`).
fn file_part(uri: &str) -> &str {
    match uri.split_once('#') {
        Some((file, _)) => file,
        None => uri,
    }
}

/// Number of leading bytes shared by two strings.
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(uri: &str, kind: SymbolKind) -> SymbolEntry {
        let rel_path = file_part(uri).strip_prefix("repo://").unwrap_or(file_part(uri));
        SymbolEntry {
            uri: SymbolUri(uri.to_string()),
            rel_path: rel_path.to_string(),
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            kind,
            signature: uri.rsplit('#').next().unwrap_or("").to_string(),
        }
    }

    fn test_index() -> SymbolIndex {
        let mut index = SymbolIndex::default();
        for (uri, kind) in [
            ("repo://crates/cw-core/src/lib.rs", SymbolKind::File),
            ("repo://crates/cw-core/src/ir.rs", SymbolKind::File),
            ("repo://crates/cw-core/src/ir.rs#ContextNode", SymbolKind::Struct),
            (
                "repo://crates/cw-core/src/lib.rs#calibration",
                SymbolKind::Module,
            ),
            (
                "repo://crates/cw-core/src/lib.rs#control",
                SymbolKind::Module,
            ),
            (
                "repo://crates/cw-core/src/lib.rs#ir",
                SymbolKind::Module,
            ),
            (
                "repo://crates/cw-core/src/lib.rs#linter",
                SymbolKind::Module,
            ),
            (
                "repo://crates/cw-core/src/lib.rs#tokenizer",
                SymbolKind::Module,
            ),
        ] {
            let value = entry(uri, kind);
            index.entries.insert(value.uri.clone(), value);
        }
        index
    }

    #[test]
    fn resolve_exact_uri() {
        let index = test_index();
        let hit = index.resolve("repo://crates/cw-core/src/lib.rs#ir").unwrap();
        assert_eq!(hit.rel_path, "crates/cw-core/src/lib.rs");
        assert_eq!(hit.kind, SymbolKind::Module);

        // Bare file URIs are first-class entries too.
        let file = index.resolve("repo://crates/cw-core/src/ir.rs").unwrap();
        assert_eq!(file.kind, SymbolKind::File);
    }

    #[test]
    fn resolve_missing_uri_returns_nearest_siblings() {
        let index = test_index();

        // Missing symbol in a known file: only that file's entries are suggested.
        let nearest = index
            .resolve("repo://crates/cw-core/src/lib.rs#ir_broken")
            .unwrap_err();
        assert_eq!(nearest.len(), 5, "capped at {MAX_RESOLVE_SUGGESTIONS}");
        for uri in &nearest {
            assert!(uri.starts_with("repo://crates/cw-core/src/lib.rs"), "{uri}");
        }
        // Closest sibling first: longest common prefix wins.
        assert_eq!(nearest[0], "repo://crates/cw-core/src/lib.rs#ir");

        // Deterministic across repeated calls.
        let again = index
            .resolve("repo://crates/cw-core/src/lib.rs#ir_broken")
            .unwrap_err();
        assert_eq!(nearest, again);

        // Missing file entirely: fall back to overall closest entries.
        let fallback = index.resolve("repo://missing/file.rs#x").unwrap_err();
        assert!(!fallback.is_empty());
        for uri in &fallback {
            assert_ne!(file_part(uri), "repo://missing/file.rs");
        }
    }
}
