//! Session-scoped virtual memory registers (Milestone B).
//!
//! [`WorkingSetState`] holds the Zone 2 registers of one session: mounted
//! file/symbol slices and pinned architectural invariants. Both maps are
//! `BTreeMap`s keyed by URI / invariant key, so every mutation and render is
//! deterministic and independent of access or insertion order — the KV-cache
//! prefix stability requirement for Zone 1/Zone 2 rendering.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::symbol_index::{SymbolIndex, SymbolKind, SymbolUri};
use crate::tokenizer::estimate_tokens;

/// How a mounted slice is materialized into Zone 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountMode {
    /// The exact byte slice of the symbol (or whole file) as indexed.
    Full,
    /// Signatures only: no function bodies. For `SymbolKind::File` entries this is
    /// every non-file symbol signature in that file joined by newlines; otherwise
    /// the single entry signature.
    Signature,
}

/// One mounted register: an exact slice of a workspace source with metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountedSlice {
    pub uri: SymbolUri,
    pub rel_path: String,
    pub mode: MountMode,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub token_estimate: usize,
}

/// Deterministic session-scoped register state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingSetState {
    /// Lexicographically sorted by URI to preserve KV-cache prefix stability
    pub mounted: BTreeMap<SymbolUri, MountedSlice>,
    /// Lexicographically sorted by invariant key
    pub invariants: BTreeMap<String, String>,
}

impl WorkingSetState {
    /// Mount `uri` (resolved through `index`) into Zone 2 and return the stored
    /// slice. The source file is read from `workspace_root.join(rel_path)`; an
    /// invalid URI propagates the resolver's nearest-URI suggestions, and a
    /// missing/unreadable file or out-of-range byte window yields a single-entry
    /// error vector. Re-mounting the same URI replaces the stored slice.
    pub fn mount(
        &mut self,
        index: &SymbolIndex,
        workspace_root: &Path,
        uri: &str,
        mode: MountMode,
    ) -> Result<&MountedSlice, Vec<String>> {
        let entry = index.resolve(uri)?;
        let source_path = workspace_root.join(&entry.rel_path);
        let source = std::fs::read_to_string(&source_path).map_err(|err| {
            vec![format!(
                "cannot read {rel_path}: {err}",
                rel_path = entry.rel_path
            )]
        })?;
        let slice_str = source
            .get(entry.start_byte..entry.end_byte)
            .ok_or_else(|| {
                vec![format!(
                    "byte range {}..{} out of bounds for {}",
                    entry.start_byte, entry.end_byte, entry.rel_path
                )]
            })?;

        let content = match mode {
            MountMode::Full => slice_str.to_string(),
            MountMode::Signature if entry.kind == SymbolKind::File => index
                .entries
                .values()
                // BTreeMap iteration: deterministic, lexicographic URI order.
                .filter(|e| e.rel_path == entry.rel_path && e.kind != SymbolKind::File)
                .map(|e| e.signature.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            MountMode::Signature => entry.signature.clone(),
        };

        let token_estimate = estimate_tokens(&content) as usize;
        let mounted = MountedSlice {
            uri: entry.uri.clone(),
            rel_path: entry.rel_path.clone(),
            mode,
            start_line: entry.start_line,
            end_line: entry.end_line,
            content,
            token_estimate,
        };
        self.mounted.insert(entry.uri.clone(), mounted);
        Ok(&self.mounted[&entry.uri])
    }

    /// Remove a mounted register; `true` when it was present.
    pub fn unmount(&mut self, uri: &str) -> bool {
        self.mounted.remove(&SymbolUri(uri.to_string())).is_some()
    }

    /// Pin (or replace) an architectural invariant under `key`.
    pub fn pin(&mut self, key: impl Into<String>, rule: impl Into<String>) {
        self.invariants.insert(key.into(), rule.into());
    }

    /// Heuristic token total of Zone 2 content: the sum of mounted slice
    /// estimates plus an estimate over each invariant key and rule text.
    pub fn total_mounted_tokens(&self) -> usize {
        let mut total: usize = self
            .mounted
            .values()
            .map(|slice| slice.token_estimate)
            .sum();
        for (key, rule) in &self.invariants {
            total += estimate_tokens(key) as usize;
            total += estimate_tokens(rule) as usize;
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch workspace under the system temp dir (unique per process AND per
    /// test label, since tests run in parallel threads); callers must drop it
    /// via `cleanup_dir`.
    fn scratch_workspace(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cw-working-set-test-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&root);
        root
    }

    fn cleanup_dir(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mount_order_is_lexicographic_regardless_of_mount_sequence() {
        let root = scratch_workspace("order");
        std::fs::write(root.join("a.rs"), "pub fn alpha() {}\n").unwrap();
        std::fs::write(root.join("b.rs"), "pub fn beta() {}\n").unwrap();
        let index = SymbolIndex::build(&root).unwrap();

        let mut state = WorkingSetState::default();
        // Mount b first, a second: storage order must still be a, b.
        assert!(
            state
                .mount(&index, &root, "repo://b.rs", MountMode::Full)
                .is_ok()
        );
        assert!(
            state
                .mount(&index, &root, "repo://a.rs", MountMode::Full)
                .is_ok()
        );

        let keys: Vec<&SymbolUri> = state.mounted.keys().collect();
        assert_eq!(
            keys.iter().map(|uri| uri.0.as_str()).collect::<Vec<_>>(),
            ["repo://a.rs", "repo://b.rs"]
        );
        cleanup_dir(&root);
    }

    #[test]
    fn signature_mode_mounts_without_bodies() {
        let root = scratch_workspace("signature");
        std::fs::write(
            root.join("b.rs"),
            "pub fn beta() {\n    let x = 1;\n    x\n}\n",
        )
        .unwrap();
        let index = SymbolIndex::build(&root).unwrap();

        // Non-file symbol: exactly its normalized signature (params kept, no body).
        let mut state = WorkingSetState::default();
        let slice = state
            .mount(&index, &root, "repo://b.rs#beta", MountMode::Signature)
            .unwrap();
        assert_eq!(slice.content, "fn beta()");
        assert!(!slice.content.contains('{'));

        // File-kind symbol: all non-file signatures in the file joined.
        let mut file_state = WorkingSetState::default();
        let slice = file_state
            .mount(&index, &root, "repo://b.rs", MountMode::Signature)
            .unwrap();
        assert_eq!(slice.content, "fn beta()");

        // Full mode keeps the exact byte slice.
        let mut full_state = WorkingSetState::default();
        let full = full_state
            .mount(&index, &root, "repo://b.rs#beta", MountMode::Full)
            .unwrap();
        assert_eq!(full.content, "pub fn beta() {\n    let x = 1;\n    x\n}");

        cleanup_dir(&root);
    }

    #[test]
    fn invalid_uri_yields_nearest_suggestions_without_panicking() {
        let root = scratch_workspace("invalid");
        std::fs::write(root.join("a.rs"), "pub fn alpha() {}\n").unwrap();
        let index = SymbolIndex::build(&root).unwrap();

        let mut state = WorkingSetState::default();
        let err = state
            .mount(&index, &root, "repo://a.rs#missing_symbol", MountMode::Full)
            .expect_err("unknown URI must fail");
        assert!(!err.is_empty(), "error carries nearest valid URIs: {err:?}");
        assert!(state.mounted.is_empty());

        // Completely unknown file still resolves to some suggestion list.
        let err = state
            .mount(&index, &root, "repo://nope.rs", MountMode::Full)
            .expect_err("unknown URI must fail");
        assert!(!err.is_empty());
        assert!(state.mounted.is_empty());
        cleanup_dir(&root);
    }

    #[test]
    fn unmount_pin_and_token_total_are_deterministic() {
        let root = scratch_workspace("ops");
        std::fs::write(root.join("a.rs"), "pub fn alpha() {}\n").unwrap();
        let index = SymbolIndex::build(&root).unwrap();

        let mut state = WorkingSetState::default();
        assert!(
            state
                .mount(&index, &root, "repo://a.rs", MountMode::Full)
                .is_ok()
        );
        let with_mount = state.total_mounted_tokens();
        assert_eq!(with_mount, estimate_tokens("pub fn alpha() {}\n") as usize);

        state.pin("kv-rule", "keep Zone 1 static");
        let total_pinned = state.total_mounted_tokens();
        assert_eq!(
            total_pinned,
            with_mount
                + estimate_tokens("kv-rule") as usize
                + estimate_tokens("keep Zone 1 static") as usize
        );

        assert!(state.unmount("repo://a.rs"));
        assert!(
            !state.unmount("repo://a.rs"),
            "second unmount reports absence"
        );
        assert!(state.mounted.is_empty());
        assert_eq!(
            state.total_mounted_tokens(),
            estimate_tokens("kv-rule") as usize + estimate_tokens("keep Zone 1 static") as usize
        );
        cleanup_dir(&root);
    }
}
