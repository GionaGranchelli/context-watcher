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
use serde_json::{Value, json};

use crate::symbol_index::{SymbolIndex, SymbolKind, SymbolUri, ZONE1_MARKER};
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

    /// Mount `uri` into Zone 2 after re-indexing its source file from disk.
    ///
    /// Freshness invariant: the file part of the URI (everything before the first
    /// `#`) is stripped of a leading `repo://` prefix, and when a non-empty path
    /// remains, its [`SymbolIndex`] entries are refreshed via
    /// [`SymbolIndex::refresh_file`] *before* any bytes are sliced, so line and
    /// byte ranges always reflect the on-disk content at mount time. Refresh IO
    /// errors are ignored (`let _`): a transient read failure must not block a
    /// mount that would otherwise resolve — the mount then proceeds against
    /// whatever index state survived (fail-open). URIs without a `repo://` file
    /// prefix, or with an empty file part, skip the refresh entirely and fall
    /// through to [`Self::mount`] semantics unchanged.
    pub fn mount_fresh(
        &mut self,
        index: &mut SymbolIndex,
        workspace_root: &Path,
        uri: &str,
        mode: MountMode,
    ) -> Result<&MountedSlice, Vec<String>> {
        let rel_path = uri
            .split_once('#')
            .map_or(uri.to_string(), |(file_part, _)| file_part.to_string())
            .strip_prefix("repo://")
            .filter(|path| !path.is_empty())
            .map(str::to_owned);
        if let Some(rel_path) = rel_path {
            let _ = index.refresh_file(workspace_root, &rel_path);
        }
        self.mount(index, workspace_root, uri, mode)
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

    /// Render the deterministic Zone 2 block, or `None` when nothing is mounted
    /// and no invariant is pinned. Mounted slices render in `BTreeMap` URI order,
    /// invariant lines in key order — never insertion or access order — so the
    /// block (and therefore every KV-cache prefix after it) is a pure function of
    /// register content:
    ///
    /// ```text
    /// <mounted_context>
    /// ### [repo://src/lib.rs#foo] (mode: full, lines: 10-25)
    /// <exact slice content>
    /// </mounted_context>
    ///
    /// <pinned_invariants>
    /// - [key]: rule text
    /// </pinned_invariants>
    /// ```
    pub fn render_zone2_block(&self) -> Option<String> {
        if self.mounted.is_empty() && self.invariants.is_empty() {
            return None;
        }

        let mut out = String::new();
        if !self.mounted.is_empty() {
            out.push_str("<mounted_context>\n");
            for (uri, slice) in &self.mounted {
                let mode = match slice.mode {
                    MountMode::Full => "full",
                    MountMode::Signature => "signature",
                };
                out.push_str(&format!(
                    "### [{}] (mode: {}, lines: {}-{})\n",
                    uri.0, mode, slice.start_line, slice.end_line
                ));
                out.push_str(&slice.content);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            out.push_str("</mounted_context>");
        }

        if !self.invariants.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str("<pinned_invariants>\n");
            for (key, rule) in &self.invariants {
                out.push_str(&format!("- [{key}]: {rule}\n"));
            }
            out.push_str("</pinned_invariants>");
        }

        Some(out)
    }
}

/// Zone 2 idempotency markers: a payload whose system prompt already carries
/// either frame is left untouched on every subsequent call. The newline anchor is
/// required: the Zone 1 protocol header *mentions* `<mounted_context>` in prose
/// ("into Zone 2 (<mounted_context>)."), so a bare tag scan would false-positive on
/// every Zone 1-injected payload. Real frames always open with `<tag>\n`.
const ZONE2_MARKERS: [&str; 2] = ["<mounted_context>\n", "<pinned_invariants>\n"];

/// Inject `zone2_block` into the outgoing system prompt (Zone 2) of an outbound
/// provider payload, immediately after Zone 1 (`<workspace_symbol_map>`) and
/// before any user/assistant/tool message, so mounting or unmounting symbols can
/// never change Zone 1's byte prefix. Returns `true` when injected, `false` when
/// the payload already carries a Zone 2 frame (idempotency), uses an unsupported
/// shape for the provider, or is outside Zone 2 scope (Gemini).
///
/// - OpenAI / OpenAI-compatible: appends `"\n\n" + zone2_block` to the first
///   `system`/`developer` string message that carries the Zone 1 marker; without
///   one, inserts a fresh system message immediately after the first existing
///   `system`/`developer` message (or prepends when none exists).
/// - Anthropic: appends to `payload["system"]`, handling both the plain-string
///   form and the array-of-text-blocks form. A payload without any `system`
///   field is left unmodified.
pub fn inject_zone2_registers(
    payload: &mut Value,
    provider: crate::ir::ProviderKind,
    zone2_block: &str,
) -> bool {
    if payload_has_zone2_marker(payload) {
        return false;
    }

    match provider {
        crate::ir::ProviderKind::OpenAI | crate::ir::ProviderKind::OpenAICompatible => {
            inject_openai_family_zone2(payload, zone2_block)
        }
        crate::ir::ProviderKind::Anthropic => inject_anthropic_zone2(payload, zone2_block),
        // Gemini Zone 2 injection is out of scope for this milestone.
        crate::ir::ProviderKind::Gemini => false,
    }
}

fn is_system_role(message: &Value) -> bool {
    message
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| role == "system" || role == "developer")
}

fn inject_openai_family_zone2(payload: &mut Value, zone2_block: &str) -> bool {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };

    // Preferred placement: right after Zone 1 in the first eligible system/developer
    // string message, so the appended registers follow the stable prefix.
    for message in messages.iter_mut() {
        if !is_system_role(message) {
            continue;
        }
        if let Some(Value::String(content)) = message.get_mut("content")
            && content.contains(ZONE1_MARKER)
        {
            *content = format!("{content}\n\n{zone2_block}");
            return true;
        }
    }

    // No Zone 1 anchor: insert a fresh system message immediately after the first
    // existing system/developer message, preserving its exact bytes.
    if let Some(position) = messages.iter().position(is_system_role) {
        messages.insert(
            position + 1,
            json!({ "role": "system", "content": zone2_block }),
        );
        return true;
    }

    messages.insert(0, json!({ "role": "system", "content": zone2_block }));
    true
}

fn inject_anthropic_zone2(payload: &mut Value, zone2_block: &str) -> bool {
    let Some(system) = payload.get_mut("system") else {
        return false; // conservative: never invent a system field.
    };
    match system {
        Value::String(text) => {
            *text = format!("{text}\n\n{zone2_block}");
            true
        }
        Value::Array(blocks) => {
            for block in blocks.iter_mut().rev() {
                if block.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(Value::String(text)) = block.get_mut("text") {
                    *text = format!("{text}\n\n{zone2_block}");
                    return true;
                }
            }
            blocks.push(json!({ "type": "text", "text": zone2_block }));
            true
        }
        _ => false, // unsupported system shape: leave the payload untouched.
    }
}

/// True when any provider-relevant system prompt location already carries a Zone 2
/// frame marker (OpenAI-family message content or Anthropic `system`).
fn payload_has_zone2_marker(payload: &Value) -> bool {
    if let Some(messages) = payload.get("messages").and_then(Value::as_array) {
        for message in messages {
            if is_system_role(message) && message_content_contains_any(message, &ZONE2_MARKERS) {
                return true;
            }
        }
    }

    match payload.get("system") {
        Some(Value::String(text)) => contains_any_marker(text, &ZONE2_MARKERS),
        Some(Value::Array(blocks)) => blocks.iter().any(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| contains_any_marker(text, &ZONE2_MARKERS))
        }),
        _ => false,
    }
}

/// String content (or any `text` part of a parts-array) containing one of the markers.
fn message_content_contains_any(message: &Value, markers: &[&str]) -> bool {
    match message.get("content") {
        Some(Value::String(text)) => contains_any_marker(text, markers),
        Some(Value::Array(parts)) => parts.iter().any(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| contains_any_marker(text, markers))
        }),
        _ => false,
    }
}

/// `true` when any marker occurs in `text`.
fn contains_any_marker(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}

/// Stub prefix shared by the eviction replacement and its idempotency check.
const ZONE3_STUB_PREFIX: &str = "[Mounted in Zone 2:";

/// Mounted-file summary used for Zone 3 scrubbing, keyed by rel_path in
/// `BTreeMap` order (the first — lexicographically smallest — mounted URI of a
/// file becomes the canonical one naming its stubs).
struct MountedFile<'a> {
    uri: &'a str,
    contents: Vec<&'a str>,
}

/// Replace historical raw file-read tool results in Zone 3 whose content
/// duplicates a file already mounted in Zone 2 with the single line
/// `[Mounted in Zone 2: <uri> — raw duplicate evicted]`, and return how many
/// tool results were scrubbed.
///
/// A result is scrubbed only when the complete text equals a mounted slice, or
/// consists of a recognized path header followed by exactly that slice. Mixed
/// output stays intact so diagnostics and other evidence are never discarded.
///
/// - OpenAI / OpenAI-compatible: `role == "tool"` messages with string content.
/// - Anthropic: `type == "tool_result"` blocks inside user messages (string
///   content or `text` parts).
/// - Gemini is out of scope for this milestone.
pub fn stub_mounted_reads_in_zone3(
    payload: &mut Value,
    provider: crate::ir::ProviderKind,
    state: &WorkingSetState,
) -> usize {
    let mut by_file: BTreeMap<String, MountedFile> = BTreeMap::new();
    for (uri, slice) in &state.mounted {
        let entry = by_file
            .entry(slice.rel_path.clone())
            .or_insert_with(|| MountedFile {
                uri: &uri.0,
                contents: Vec::new(),
            });
        if !slice.content.is_empty() {
            entry.contents.push(&slice.content);
        }
    }
    if by_file.is_empty() {
        return 0;
    }

    let mut scrubbed = 0usize;
    match provider {
        crate::ir::ProviderKind::OpenAI | crate::ir::ProviderKind::OpenAICompatible => {
            if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages.iter_mut() {
                    let is_tool = message
                        .get("role")
                        .and_then(Value::as_str)
                        .is_some_and(|role| role == "tool");
                    if !is_tool {
                        continue;
                    }
                    if let Some(Value::String(text)) = message.get_mut("content")
                        && scrub_text(text, &by_file)
                    {
                        scrubbed += 1;
                    }
                }
            }
        }
        crate::ir::ProviderKind::Anthropic => {
            if let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) {
                for message in messages.iter_mut() {
                    // tool_result blocks live inside user-message content arrays:
                    // block.content is a string or an array of text parts.
                    let Some(blocks) = message
                        .get("role")
                        .and_then(Value::as_str)
                        .is_some_and(|role| role == "user")
                        .then(|| message.get_mut("content"))
                        .flatten()
                        .and_then(Value::as_array_mut)
                    else {
                        continue;
                    };
                    for block in blocks.iter_mut() {
                        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                            continue;
                        }
                        let mut scrubbed_block = false;
                        match block.get_mut("content") {
                            Some(Value::String(text)) => {
                                scrubbed_block = scrub_text(text, &by_file);
                            }
                            Some(Value::Array(parts)) => {
                                for part in parts.iter_mut() {
                                    if part.get("type").and_then(Value::as_str) == Some("text")
                                        && let Some(Value::String(text)) = part.get_mut("text")
                                        && scrub_text(text, &by_file)
                                    {
                                        scrubbed_block = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                        if scrubbed_block {
                            scrubbed += 1;
                        }
                    }
                }
            }
        }
        crate::ir::ProviderKind::Gemini => {}
    }
    scrubbed
}

/// In-place stub replacement only when the entire text region is redundant.
fn scrub_text(text: &mut String, by_file: &BTreeMap<String, MountedFile>) -> bool {
    if text.is_empty() || text.starts_with(ZONE3_STUB_PREFIX) {
        return false; // already stubbed (or nothing to match).
    }
    let (body, hud) = match text.rsplit_once('\n') {
        Some((body, hud)) if hud.starts_with("[cw:") && hud.ends_with(']') => (body, Some(hud)),
        _ => (text.as_str(), None),
    };
    for (rel_path, file) in by_file {
        let duplicate = file.contents.iter().any(|content| {
            let candidates = [
                (*content).to_string(),
                format!("Contents of {rel_path}:\n{content}"),
                format!("file {rel_path}:\n{content}"),
                format!("{rel_path}:\n{content}"),
            ];
            candidates.iter().any(|candidate| {
                body == candidate
                    || body
                        .strip_suffix('\n')
                        .is_some_and(|without_newline| without_newline == candidate)
            })
        });
        if duplicate {
            let stub = format!("[Mounted in Zone 2: {} — raw duplicate evicted]", file.uri);
            *text = hud.map_or(stub.clone(), |hud| format!("{stub}\n{hud}"));
            return true;
        }
    }
    false
}

/// Append the one-line budget HUD to the tail of the final user/tool message:
/// `[cw: {active}/{budget} tok | mounted: N | pinned: M | zone1_prefix_bytes: B]`.
/// Idempotent: when the content already ends with a `[cw:` line it is replaced,
/// never stacked — so repeated calls (or a changed budget between turns) leave
/// exactly one HUD line. No-op when the provider/payload has no eligible message.
/// Units are display heuristics, not provider-billed tokens: `active_tokens` is
/// estimated over the serialized JSON payload (keys, escapes and metadata
/// included), while `budget_tokens` is a fixed reference value supplied by the
/// caller — not per-model or per-session.
pub fn append_budget_hud(
    payload: &mut Value,
    provider: crate::ir::ProviderKind,
    active_tokens: usize,
    budget_tokens: usize,
    state: &WorkingSetState,
    zone1_bytes: usize,
) {
    let hud = format!(
        "[cw: {active_tokens}/{budget_tokens} tok | mounted: {} | pinned: {} | zone1_prefix_bytes: {}]",
        state.mounted.len(),
        state.invariants.len(),
        zone1_bytes
    );

    match provider {
        crate::ir::ProviderKind::OpenAI | crate::ir::ProviderKind::OpenAICompatible => {
            let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
                return;
            };
            let Some(index) = messages.iter().rposition(|message| {
                message
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|role| role == "user" || role == "tool")
            }) else {
                return;
            };
            if let Some(Value::String(text)) = messages[index].get_mut("content") {
                *text = with_hud_line(text, &hud);
            }
        }
        crate::ir::ProviderKind::Anthropic => {
            let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
                return;
            };
            let Some(index) = messages
                .iter()
                .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            else {
                return;
            };
            match messages[index].get_mut("content") {
                Some(Value::String(text)) => *text = with_hud_line(text, &hud),
                Some(Value::Array(parts)) => {
                    for part in parts.iter_mut().rev() {
                        if part.get("type").and_then(Value::as_str) == Some("text")
                            && let Some(Value::String(text)) = part.get_mut("text")
                        {
                            *text = with_hud_line(text, &hud);
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
        crate::ir::ProviderKind::Gemini => {}
    }
}

/// Content with a single trailing HUD line (existing `[cw:` tail replaced).
fn with_hud_line(text: &str, hud: &str) -> String {
    let body = if text.ends_with('\n') && !text.is_empty() {
        &text[..text.len() - 1]
    } else {
        text
    };
    match body.rfind('\n') {
        Some(pos) => {
            let tail = &body[pos + 1..];
            if tail.starts_with("[cw:") {
                let prefix = &body[..pos]; // replace the stale HUD line
                format!("{prefix}\n{hud}")
            } else {
                format!("{body}\n{hud}")
            }
        }
        None => {
            if body.starts_with("[cw:") {
                hud.to_string()
            } else {
                format!("{body}\n{hud}")
            }
        }
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

    // ----- Task 2: Zone 2 renderer & prefix-safe injector tests -----

    fn test_slice(uri: &str, content: &str, start_line: usize, end_line: usize) -> MountedSlice {
        MountedSlice {
            uri: SymbolUri(uri.to_string()),
            rel_path: "src/lib.rs".to_string(),
            mode: MountMode::Full,
            start_line,
            end_line,
            content: content.to_string(),
            token_estimate: estimate_tokens(content) as usize,
        }
    }

    fn test_zone1_block() -> String {
        let index = SymbolIndex::build_from_sources(&[(
            "src/lib.rs",
            "pub fn foo() {}\npub fn bar(x: u32) -> u32 {\n    x\n}\n",
        )]);
        index.render_zone1_prefix(512)
    }

    #[test]
    fn render_zone2_is_deterministic_and_sorted_by_key() {
        // Two states filled in different orders must render identically.
        let mut a = WorkingSetState::default();
        a.mounted.insert(
            SymbolUri("repo://src/lib.rs#beta".into()),
            test_slice("repo://src/lib.rs#beta", "fn beta()", 1, 4),
        );
        a.mounted.insert(
            SymbolUri("repo://src/lib.rs#alpha".into()),
            test_slice("repo://src/lib.rs#alpha", "fn alpha()", 5, 9),
        );
        a.pin("zeta-rule", "last pinned first");
        a.pin("alpha-rule", "first invariant");

        let mut b = WorkingSetState::default();
        b.mounted.insert(
            SymbolUri("repo://src/lib.rs#alpha".into()),
            test_slice("repo://src/lib.rs#alpha", "fn alpha()", 5, 9),
        );
        b.mounted.insert(
            SymbolUri("repo://src/lib.rs#beta".into()),
            test_slice("repo://src/lib.rs#beta", "fn beta()", 1, 4),
        );
        b.pin("alpha-rule", "first invariant");
        b.pin("zeta-rule", "last pinned first");

        let ra = a.render_zone2_block().expect("non-empty state renders");
        let rb = b.render_zone2_block().expect("non-empty state renders");
        assert_eq!(ra, rb);

        // Exact frame layout: URI-sorted entries, then key-sorted invariants.
        assert_eq!(
            ra,
            "<mounted_context>\n\
             ### [repo://src/lib.rs#alpha] (mode: full, lines: 5-9)\n\
             fn alpha()\n\
             ### [repo://src/lib.rs#beta] (mode: full, lines: 1-4)\n\
             fn beta()\n\
             </mounted_context>\n\n\
             <pinned_invariants>\n\
             - [alpha-rule]: first invariant\n\
             - [zeta-rule]: last pinned first\n\
             </pinned_invariants>"
        );

        assert!(WorkingSetState::default().render_zone2_block().is_none());
    }

    #[test]
    fn zone1_prefix_bytes_are_unchanged_by_zone2_mounts_and_unmounts() {
        let payload = json!({
            "model": "gpt-test",
            "messages": [
                {"role": "system", "content": "base system prompt"},
                {"role": "user", "content": "hello"}
            ]
        });

        let mut payload1 = payload.clone();
        assert!(crate::symbol_index::inject_zone1_prefix(
            &mut payload1,
            crate::ir::ProviderKind::OpenAI,
            &test_zone1_block()
        ));
        let system_before = payload1["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned();

        // Mount symbols (out of order) and pin invariants; Zone 2 must land after
        // Zone 1 without touching a single byte before or inside it.
        let mut state = WorkingSetState::default();
        state.mounted.insert(
            SymbolUri("repo://src/lib.rs#bar".into()),
            test_slice("repo://src/lib.rs#bar", "fn bar(x: u32) -> u32", 2, 4),
        );
        state.mounted.insert(
            SymbolUri("repo://src/lib.rs#foo".into()),
            test_slice("repo://src/lib.rs#foo", "fn foo()", 1, 1),
        );
        state.pin("kv-rule", "keep Zone 1 static");

        let zone2 = state.render_zone2_block().expect("registers render");
        assert!(inject_zone2_registers(
            &mut payload1,
            crate::ir::ProviderKind::OpenAI,
            &zone2
        ));
        let system_after = payload1["messages"][0]["content"].as_str().unwrap();

        // Zone 1 byte prefix (through its closing tag) is identical before/after.
        let zone1_end = system_before.find("</workspace_symbol_map>").unwrap()
            + "</workspace_symbol_map>".len();
        assert_eq!(
            &system_after[..zone1_end],
            &system_before[..zone1_end],
            "Zone 1 byte prefix must be untouched by Zone 2 injection"
        );
        assert!(system_after.starts_with(system_before.as_str()));
        let z2_pos = system_after.find("<mounted_context>\n").unwrap();
        assert!(z2_pos > zone1_end, "Zone 2 must follow the Zone 1 frame");

        // Unmounting everything renders nothing new; a later mount cannot re-inject
        // over an existing frame either (idempotency).
        state.unmount("repo://src/lib.rs#foo");
        assert!(state.render_zone2_block().is_some()); // invariants still pinned
        let again = state.render_zone2_block().unwrap();
        assert!(
            !inject_zone2_registers(&mut payload1, crate::ir::ProviderKind::OpenAI, &again),
            "second injection must be rejected (frame already present)"
        );

        // Unmounting the remaining register leaves only invariants: still a block.
        state.unmount("repo://src/lib.rs#bar");
        let invariants_only = state.render_zone2_block().expect("invariants render alone");
        assert!(!invariants_only.contains("<mounted_context>"));
    }

    #[test]
    fn zone2_double_injection_is_idempotent() {
        let payload = json!({
            "messages": [
                {"role": "system", "content": "base"},
                {"role": "user", "content": "hi"}
            ]
        });
        let mut p = payload.clone();
        assert!(inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::OpenAICompatible,
            "<mounted_context>\n### [repo://x.rs#a] (mode: full, lines: 1-2)\nfn a()\n</mounted_context>"
        ));
        let snapshot = serde_json::to_string(&p).unwrap();
        assert!(
            !inject_zone2_registers(
                &mut p,
                crate::ir::ProviderKind::OpenAICompatible,
                "<mounted_context>\n### [repo://y.rs#b] (mode: full, lines: 1-3)\nfn b()\n</mounted_context>"
            ),
            "payload already carries a Zone 2 frame"
        );
        assert_eq!(serde_json::to_string(&p).unwrap(), snapshot);
    }

    #[test]
    fn zone2_anthropic_handles_string_array_and_missing_system() {
        let block = "<mounted_context>\n### [repo://x.rs#a] (mode: full, lines: 1-2)\nfn a()\n</mounted_context>";

        // String system with Zone 1 already present: appended right after it.
        let base_system = format!("base\n\ncw memory protocol + {ZONE1_MARKER} frame");
        let mut p = json!({ "system": base_system.clone(), "messages": [] });
        assert!(inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::Anthropic,
            block
        ));
        let system = p["system"].as_str().unwrap();
        assert!(system.starts_with(base_system.as_str()));
        assert!(system.ends_with(block));

        // Array-of-text-blocks system: appended into the last text block.
        let mut p = json!({
            "system": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": format!("zone1 {ZONE1_MARKER}")},
            ],
            "messages": []
        });
        assert!(inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::Anthropic,
            block
        ));
        let last = p["system"][1]["text"].as_str().unwrap();
        assert_eq!(last, format!("zone1 {ZONE1_MARKER}\n\n{block}"));
        assert_eq!(p["system"][0]["text"], "first");

        // No system field: conservative no-op.
        let mut p = json!({ "messages": [] });
        let snapshot = serde_json::to_string(&p).unwrap();
        assert!(!inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::Anthropic,
            block
        ));
        assert_eq!(serde_json::to_string(&p).unwrap(), snapshot);
    }

    #[test]
    fn zone2_openai_fallbacks_preserve_existing_system_bytes() {
        let block = "<pinned_invariants>\n- [k]: r\n</pinned_invariants>";

        // Existing system without Zone 1: fresh system message inserted right after it.
        let mut p = json!({ "messages": [
            {"role": "system", "content": "original"},
            {"role": "user", "content": "hi"}
        ]});
        assert!(inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::OpenAI,
            block
        ));
        let messages = p["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(
            messages[0]["content"], "original",
            "first system bytes preserved"
        );
        assert_eq!(messages[1]["role"], "system");
        assert_eq!(messages[1]["content"], block);

        // No system message at all: prepended.
        let mut p = json!({ "messages": [ {"role": "user", "content": "hi"} ] });
        assert!(inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::OpenAI,
            block
        ));
        let messages = p["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], block);

        // Gemini is out of scope: untouched.
        let mut p = json!({ "system": "base", "messages": [] });
        let snapshot = serde_json::to_string(&p).unwrap();
        assert!(!inject_zone2_registers(
            &mut p,
            crate::ir::ProviderKind::Gemini,
            block
        ));
        assert_eq!(serde_json::to_string(&p).unwrap(), snapshot);
    }

    // ----- Task 3: Zone 3 stubbing & budget HUD tests -----

    const DUMP: &str = "pub fn foo() {\n    let answer = 42;\n    answer\n}\n";

    fn state_with_one_mount() -> WorkingSetState {
        let mut state = WorkingSetState::default();
        state.mounted.insert(
            SymbolUri("repo://src/lib.rs#foo".into()),
            test_slice("repo://src/lib.rs#foo", DUMP, 1, 4),
        );
        state.pin("kv-rule", "keep Zone 1 static");
        state
    }

    #[test]
    fn stubbed_mounted_read_collapses_and_unmounted_outputs_stay() {
        let state = state_with_one_mount();
        let unrelated = "total 8\ndrwxr-xr-x 2 u g\n-rw-r--r-- 1 u g";
        let mut p = json!({ "messages": [
            {"role": "system", "content": "s"},
            {"role": "user", "content": "read the file"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]},
            {"role": "tool", "tool_call_id": "c1", "content": format!("Contents of src/lib.rs:\n{DUMP}")},
            {"role": "tool", "tool_call_id": "c2", "content": unrelated}
        ]});

        let scrubbed = stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::OpenAI, &state);
        assert_eq!(scrubbed, 1, "only the mounted file's raw dump is evicted");
        assert_eq!(
            p["messages"][3]["content"],
            "[Mounted in Zone 2: repo://src/lib.rs#foo — raw duplicate evicted]"
        );
        assert_eq!(p["messages"][4]["content"], unrelated);

        // Already-stubbed results are never re-counted or rewritten.
        let again = stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::OpenAI, &state);
        assert_eq!(again, 0);
    }

    #[test]
    fn stale_path_header_and_mixed_tool_output_are_preserved() {
        let mut state = WorkingSetState::default();
        state.mounted.insert(
            SymbolUri("repo://src/lib.rs#other".into()),
            test_slice("repo://src/lib.rs#other", "fn other()", 9, 12),
        );
        let mut p = json!({ "messages": [
            {"role": "tool", "content": "src/lib.rs:\npub fn stale() {}\n"},
            // A path mentioned mid-text is NOT a header dump: stays untouched.
            {"role": "tool", "content": "grep hit in src/lib.rs at line 3"},
            {"role": "tool", "content": format!("Contents of src/lib.rs:\n{DUMP}\nerror: type mismatch")},
            {"role": "tool", "content": format!("Contents of src/lib.rs:\n{DUMP}\nwarning: unused variable")},
            {"role": "tool", "content": format!("Contents of src/lib.rs:\n{DUMP}\nContents of other.rs:\nfn other() {{}}")}
        ]});

        let scrubbed = stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::OpenAI, &state);
        assert_eq!(scrubbed, 0);
        assert_eq!(
            p["messages"][0]["content"],
            "src/lib.rs:\npub fn stale() {}\n"
        );
        assert_eq!(
            p["messages"][2]["content"],
            format!("Contents of src/lib.rs:\n{DUMP}\nerror: type mismatch")
        );
        assert_eq!(
            p["messages"][3]["content"],
            format!("Contents of src/lib.rs:\n{DUMP}\nwarning: unused variable")
        );
        assert!(
            p["messages"][4]["content"]
                .as_str()
                .unwrap()
                .contains("Contents of other.rs:")
        );
    }

    #[test]
    fn anthropic_tool_result_blocks_are_stubbed_in_both_shapes() {
        let state = state_with_one_mount();
        let mut p = json!({ "messages": [
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": DUMP}]},
            {"role": "user", "content": [
                {"type": "text", "text": "note"},
                {"type": "tool_result", "tool_use_id": "t2", "content": [{"type": "text", "text": format!("file src/lib.rs:\n{DUMP}")}]},
                {"type": "tool_result", "tool_use_id": "t3", "content": "unrelated output"}
            ]}
        ]});

        let scrubbed =
            stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::Anthropic, &state);
        assert_eq!(scrubbed, 2, "one per evicted tool_result block");
        assert_eq!(
            p["messages"][0]["content"][0]["content"],
            "[Mounted in Zone 2: repo://src/lib.rs#foo — raw duplicate evicted]"
        );
        let blocks = &p["messages"][1]["content"];
        assert_eq!(
            blocks[0]["text"], "note",
            "non-tool_result blocks untouched"
        );
        assert_eq!(
            blocks[1]["content"][0]["text"],
            "[Mounted in Zone 2: repo://src/lib.rs#foo — raw duplicate evicted]"
        );
        assert_eq!(blocks[2]["content"], "unrelated output");

        let again = stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::Anthropic, &state);
        assert_eq!(again, 0);
    }

    #[test]
    fn budget_hud_appends_exact_line_and_replaces_on_repeat() {
        let state = state_with_one_mount(); // mounted: 1, pinned: 1
        let mut p = json!({ "messages": [
            {"role": "system", "content": "s"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "ok"},
            {"role": "tool", "content": "result text"}
        ]});

        append_budget_hud(
            &mut p,
            crate::ir::ProviderKind::OpenAI,
            123,
            500,
            &state,
            97,
        );
        let hud = "[cw: 123/500 tok | mounted: 1 | pinned: 1 | zone1_prefix_bytes: 97]";
        assert_eq!(p["messages"][3]["content"], format!("result text\n{hud}"));
        assert_eq!(
            p["messages"][1]["content"], "hi",
            "HUD lands on the final user/tool message"
        );

        // Repeat with a changed budget: replaces, never stacks.
        append_budget_hud(
            &mut p,
            crate::ir::ProviderKind::OpenAI,
            200,
            500,
            &state,
            97,
        );
        let content = p["messages"][3]["content"].as_str().unwrap();
        let hud_lines: Vec<&str> = content
            .lines()
            .filter(|line| line.starts_with("[cw:"))
            .collect();
        assert_eq!(hud_lines.len(), 1, "exactly one HUD line after repeat");
        assert_eq!(
            hud_lines[0],
            "[cw: 200/500 tok | mounted: 1 | pinned: 1 | zone1_prefix_bytes: 97]"
        );

        // Anthropic string content on the final user message.
        let mut p = json!({ "messages": [
            {"role": "assistant", "content": [{"type": "text", "text": "done"}]},
            {"role": "user", "content": "next turn"}
        ]});
        append_budget_hud(
            &mut p,
            crate::ir::ProviderKind::Anthropic,
            10,
            500,
            &state,
            97,
        );
        assert_eq!(
            p["messages"][1]["content"],
            "next turn\n[cw: 10/500 tok | mounted: 1 | pinned: 1 | zone1_prefix_bytes: 97]"
        );

        // Anthropic parts-array user message: HUD goes into the last text part.
        let mut p = json!({ "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "first"},
                {"type": "tool_result", "tool_use_id": "t1", "content": "x"},
                {"type": "text", "text": "last"}
            ]}
        ]});
        append_budget_hud(
            &mut p,
            crate::ir::ProviderKind::Anthropic,
            10,
            500,
            &state,
            97,
        );
        assert_eq!(p["messages"][0]["content"][0]["text"], "first");
        assert_eq!(
            p["messages"][0]["content"][2]["text"],
            "last\n[cw: 10/500 tok | mounted: 1 | pinned: 1 | zone1_prefix_bytes: 97]"
        );
    }

    #[test]
    fn empty_state_scrubs_nothing_and_hud_is_still_appended() {
        let state = WorkingSetState::default();
        let mut p = json!({ "messages": [
            {"role": "tool", "content": DUMP},
        ]});
        assert_eq!(
            stub_mounted_reads_in_zone3(&mut p, crate::ir::ProviderKind::OpenAI, &state),
            0
        );
        assert_eq!(p["messages"][0]["content"], DUMP);

        append_budget_hud(&mut p, crate::ir::ProviderKind::OpenAI, 5, 100, &state, 0);
        let content = p["messages"][0]["content"].as_str().unwrap();
        assert!(
            content.ends_with("[cw: 5/100 tok | mounted: 0 | pinned: 0 | zone1_prefix_bytes: 0]")
        );
    }

    // ----- Milestone C: fresh per-file re-indexing -----

    #[test]
    fn mount_fresh_picks_up_mid_session_file_edits() {
        let root = std::env::temp_dir().join(format!("cw_mount_fresh_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn target_fn() -> u32 {\n    42\n}\n",
        )
        .unwrap();

        let mut index = SymbolIndex::build(&root).unwrap();
        assert_eq!(
            index
                .resolve("repo://src/lib.rs#target_fn")
                .unwrap()
                .start_line,
            1
        );
        assert!(
            !index
                .entries
                .contains_key(&SymbolUri("repo://src/lib.rs#helper".into())),
            "pre-edit index must not know the helper added below"
        );

        // Mid-session edit: new items above shift target_fn's line numbers and its
        // body changes — the on-disk bytes are genuinely different.
        std::fs::write(
            root.join("src/lib.rs"),
            "pub const OFFSET: u32 = 7;\n\npub fn helper() -> u8 {\n    1\n}\n\npub fn target_fn() -> u32 {\n    OFFSET + 35\n}\n",
        )
        .unwrap();

        let mut state = WorkingSetState::default();
        // The stale index still resolves the pre-edit byte window: wrong content.
        let stale = state
            .mount(
                &index,
                &root,
                "repo://src/lib.rs#target_fn",
                MountMode::Full,
            )
            .expect("stale mount still resolves");
        assert_eq!(stale.start_line, 1);
        assert!(!stale.content.contains("OFFSET + 35"));

        // The fresh mount re-indexes the file from disk before slicing.
        let fresh = state
            .mount_fresh(
                &mut index,
                &root,
                "repo://src/lib.rs#target_fn",
                MountMode::Full,
            )
            .expect("fresh mount resolves");
        assert_eq!(fresh.start_line, 7);
        assert_eq!(fresh.end_line, 9);
        assert_eq!(
            fresh.content,
            "pub fn target_fn() -> u32 {\n    OFFSET + 35\n}"
        );

        // Declarations added after the index was built are indexed and mountable.
        let helper = state
            .mount_fresh(
                &mut index,
                &root,
                "repo://src/lib.rs#helper",
                MountMode::Full,
            )
            .expect("new symbol resolvable after refresh");
        assert_eq!(helper.start_line, 3);
        assert_eq!(helper.end_line, 5);
        assert_eq!(helper.content, "pub fn helper() -> u8 {\n    1\n}");

        // Signature mode on the refreshed file exposes the new declaration's header
        // (signatures normalize away leading modifiers such as `pub`).
        let sig = state
            .mount_fresh(&mut index, &root, "repo://src/lib.rs", MountMode::Signature)
            .expect("file URI mounts in signature mode");
        assert!(sig.content.contains("fn helper() -> u8"));
        assert!(sig.content.contains("fn target_fn() -> u32"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mount_fresh_cannot_bypass_symbol_index_path_exclusions() {
        let root = std::env::temp_dir().join(format!("cw_mount_exclusions_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["src", ".github", "node_modules/pkg", "target"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (path, secret) in [
            (".env", "ENV_SECRET_DO_NOT_EXPOSE"),
            (".github/private-config.json", "CONFIG_SECRET_DO_NOT_EXPOSE"),
            (
                "node_modules/pkg/file.ts",
                "DEPENDENCY_SECRET_DO_NOT_EXPOSE",
            ),
            ("target/generated.rs", "BUILD_SECRET_DO_NOT_EXPOSE"),
            ("secrets.txt", "TEXT_SECRET_DO_NOT_EXPOSE"),
        ] {
            std::fs::write(root.join(path), secret).unwrap();
        }
        std::fs::write(root.join("src/lib.rs"), "pub fn safe() {}\n").unwrap();

        let mut index = SymbolIndex::build(&root).unwrap();
        assert_eq!(index.entries.len(), 2);
        let mut state = WorkingSetState::default();
        for path in [
            ".env",
            ".github/private-config.json",
            "node_modules/pkg/file.ts",
            "target/generated.rs",
            "secrets.txt",
        ] {
            assert!(
                state
                    .mount_fresh(
                        &mut index,
                        &root,
                        &format!("repo://{path}"),
                        MountMode::Full
                    )
                    .is_err(),
                "direct mount should reject {path}"
            );
        }
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(!serialized.contains("SECRET_DO_NOT_EXPOSE"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
