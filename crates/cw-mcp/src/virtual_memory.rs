//! Agent-controlled virtual memory syscalls (Milestone C): `cw_mount`,
//! `cw_unmount` and `cw_pin`.
//!
//! These tools mutate a session's Zone 2 registers
//! ([`cw_core::working_set::WorkingSetState`]) directly against the shared
//! [`CwStore`] — the *same database file and session space* as the proxy daemon
//! (the daemon hands its own store handle into this context, see `crates/cw-cli`)
//! — so every change is visible on the next proxied turn without a daemon
//! round-trip.
//!
//! The workspace symbol index is one in-memory lazily built `Mutex<SymbolIndex>`
//! shared across all sessions: it holds read-only *content* derived from disk,
//! while register state stays strictly per-session inside the store, so sessions
//! cannot cross-contaminate registers through the shared index. Every mount
//! re-indexes its target file from disk before slicing bytes (freshness
//! invariant, `WorkingSetState::mount_fresh`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};
use cw_core::symbol_index::SymbolIndex;
use cw_core::working_set::MountMode;
use cw_store::CwStore;
use serde_json::Value;

/// Shared context for the virtual memory tools: one store handle (the same DB
/// file as the proxy daemon) plus a lazily built workspace index. The index is
/// global and read-only content; working-set state is per-session in the store.
pub struct VirtualMemoryContext {
    pub store: CwStore,
    /// Lazily initialized on first virtual-memory tool call (the workspace walk
    /// cost is paid once, under the lock).
    index: Mutex<Option<SymbolIndex>>,
    workspace_root: PathBuf,
}

impl VirtualMemoryContext {
    /// Wrap a daemon-provided store handle. The workspace root comes from the
    /// `CW_WORKSPACE_ROOT` environment variable when set and non-empty, otherwise
    /// from the current directory; only if that also fails is the root an empty
    /// path (indexing then fails with a clear error rather than guessing).
    pub fn new(store: CwStore) -> Self {
        let workspace_root = std::env::var_os("CW_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        Self {
            store,
            index: Mutex::new(None),
            workspace_root,
        }
    }

    /// Wrap a daemon-provided store handle with an explicit workspace root.
    pub fn with_workspace_root(store: CwStore, workspace_root: PathBuf) -> Self {
        Self {
            store,
            index: Mutex::new(None),
            workspace_root,
        }
    }

    /// Workspace root this context resolves `repo://` URIs against.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

/// Dispatch one virtual memory tool call. Both success and "URI not found"
/// outcomes return `Ok(text)` — the MCP transport must never fail just because a
/// URI is unknown; the agent needs the nearest-URI suggestions to self-correct.
/// Hard errors (missing arguments, invalid mode, store/index IO failures) are
/// `Err` and surface as an MCP error result.
pub fn execute_virtual_memory_tool(
    vm: &VirtualMemoryContext,
    tool_name: &str,
    args: &Value,
) -> Result<String> {
    match tool_name {
        "cw_mount" => mount_tool(vm, args),
        "cw_unmount" => unmount_tool(vm, args),
        "cw_pin" => pin_tool(vm, args),
        other => bail!("Unknown tool: {other}"),
    }
}

/// Session resolution chain: explicit `session_id` argument > `CW_SESSION_ID`
/// environment variable > `"default"`. Explicit values are validated with the
/// same rule as every other MCP tool.
fn resolve_session_id(args: &Value) -> Result<String> {
    if let Some(id) = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !cw_core::control::is_valid_session_id(id) {
            bail!("Invalid session ID");
        }
        return Ok(id.to_string());
    }
    if let Ok(env_id) = std::env::var("CW_SESSION_ID") {
        let trimmed = env_id.trim();
        if !trimmed.is_empty() {
            if !cw_core::control::is_valid_session_id(trimmed) {
                bail!("Invalid session ID");
            }
            return Ok(trimmed.to_string());
        }
    }
    Ok("default".to_string())
}

fn required_string(args: &Value, field: &str) -> Result<String> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{field} is required"))
}

/// Lock the shared index, building it on first use. A poisoned lock (a prior
/// call panicked while holding it) is recovered rather than propagated: the
/// index is rebuildable content and must not wedge every subsequent tool call.
fn with_index<R>(
    vm: &VirtualMemoryContext,
    f: impl FnOnce(&mut SymbolIndex) -> R,
) -> std::io::Result<R> {
    let mut guard = vm
        .index
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(SymbolIndex::build(vm.workspace_root())?);
    }
    Ok(f(guard.as_mut().expect("index just initialized")))
}

fn mount_tool(vm: &VirtualMemoryContext, args: &Value) -> Result<String> {
    let uri = required_string(args, "uri")?;
    let mode_raw = args.get("mode").and_then(|v| v.as_str()).unwrap_or("full");
    let mode = match mode_raw {
        "full" => MountMode::Full,
        "signature" => MountMode::Signature,
        other => bail!("Invalid mode '{other}': expected 'full' or 'signature'"),
    };
    let session_id = resolve_session_id(args)?;

    let mut state = vm.store.get_working_set(&session_id)?;
    // Freshness invariant: re-index the target file from disk before slicing.
    let mounted = with_index(vm, |index| {
        state
            .mount_fresh(index, vm.workspace_root(), &uri, mode)
            .map(|slice| (slice.start_line, slice.end_line, slice.token_estimate))
    })?;

    match mounted {
        Ok((start_line, end_line, tokens)) => {
            vm.store.save_working_set(&session_id, &state)?;
            Ok(format!(
                "Mounted {uri} (mode: {mode_raw}, lines: {start_line}-{end_line}, ~{tokens} tokens) into Zone 2."
            ))
        }
        Err(suggestions) => {
            let list = suggestions
                .iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!("URI not found: {uri}. Nearest valid URIs:\n{list}"))
        }
    }
}

fn unmount_tool(vm: &VirtualMemoryContext, args: &Value) -> Result<String> {
    let uri = required_string(args, "uri")?;
    let session_id = resolve_session_id(args)?;

    let mut state = vm.store.get_working_set(&session_id)?;
    if !state.unmount(&uri) {
        return Ok(format!("URI was not mounted: {uri}."));
    }
    // Persist only when the register actually changed.
    vm.store.save_working_set(&session_id, &state)?;
    Ok(format!("Unmounted {uri} from Zone 2."))
}

fn pin_tool(vm: &VirtualMemoryContext, args: &Value) -> Result<String> {
    let key = required_string(args, "key")?;
    let rule = required_string(args, "rule")?;
    let session_id = resolve_session_id(args)?;

    let mut state = vm.store.get_working_set(&session_id)?;
    state.pin(&key, rule);
    vm.store.save_working_set(&session_id, &state)?;
    Ok(format!("Pinned invariant [{key}] in Zone 2."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::symbol_index::SymbolUri;
    use serde_json::json;

    const SOURCE_V1: &str = "pub fn target_fn() -> u32 {\n    42\n}\n";

    // Tests run concurrently in one process: give every context a unique temp dir.
    static DIR_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Temp workspace with `src/lib.rs` (v1) + in-memory store context.
    fn fresh_context() -> (std::path::PathBuf, VirtualMemoryContext) {
        let n = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("cw_vm_mcp_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), SOURCE_V1).unwrap();
        let store = CwStore::open_in_memory().unwrap();
        let context = VirtualMemoryContext::with_workspace_root(store, root.clone());
        (root, context)
    }

    #[test]
    fn mount_persists_to_store_with_exact_confirmation_string() {
        let (root, vm) = fresh_context();
        let out = execute_virtual_memory_tool(
            &vm,
            "cw_mount",
            &json!({"uri": "repo://src/lib.rs#target_fn", "mode": "full"}),
        )
        .unwrap();

        // Exact confirmation string, with the estimate computed from the slice.
        let state = vm.store.get_working_set("default").unwrap();
        let uri = SymbolUri("repo://src/lib.rs#target_fn".into());
        let slice = &state.mounted[&uri];
        assert_eq!(
            out,
            format!(
                "Mounted repo://src/lib.rs#target_fn (mode: full, lines: {}-{}, ~{} tokens) into Zone 2.",
                slice.start_line, slice.end_line, slice.token_estimate
            )
        );

        // SQLite round-trip through the real store API: state persisted under
        // the "default" session.
        assert_eq!(slice.content, SOURCE_V1.trim_end());
        let reloaded = vm.store.get_working_set("default").unwrap();
        assert_eq!(reloaded, state);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mount_invalid_uri_returns_nearest_suggestions_without_error() {
        let (root, vm) = fresh_context();
        let out = execute_virtual_memory_tool(
            &vm,
            "cw_mount",
            &json!({"uri": "repo://src/lib.rs#missing_fn"}),
        )
        .unwrap();

        assert!(
            out.starts_with("URI not found: repo://src/lib.rs#missing_fn. Nearest valid URIs:\n- "),
            "{out}"
        );
        assert!(out.contains("- repo://src/lib.rs"), "{out}");
        assert!(out.contains("#target_fn"), "{out}");

        // Nothing persisted for a failed mount; the transport result was Ok.
        let state = vm.store.get_working_set("default").unwrap();
        assert!(state.mounted.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unmount_persists_only_when_changed() {
        let (root, vm) = fresh_context();
        execute_virtual_memory_tool(
            &vm,
            "cw_mount",
            &json!({"uri": "repo://src/lib.rs#target_fn"}),
        )
        .unwrap();

        let out = execute_virtual_memory_tool(
            &vm,
            "cw_unmount",
            &json!({"uri": "repo://src/lib.rs#target_fn"}),
        )
        .unwrap();
        assert_eq!(out, "Unmounted repo://src/lib.rs#target_fn from Zone 2.");
        assert!(
            vm.store
                .get_working_set("default")
                .unwrap()
                .mounted
                .is_empty()
        );

        // Second unmount: nothing to remove.
        let again = execute_virtual_memory_tool(
            &vm,
            "cw_unmount",
            &json!({"uri": "repo://src/lib.rs#target_fn"}),
        )
        .unwrap();
        assert_eq!(again, "URI was not mounted: repo://src/lib.rs#target_fn.");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pin_persists_invariant_for_the_session() {
        let (root, vm) = fresh_context();
        let out = execute_virtual_memory_tool(
            &vm,
            "cw_pin",
            &json!({"key": "arch", "rule": "no unwrap in proxy"}),
        )
        .unwrap();
        assert_eq!(out, "Pinned invariant [arch] in Zone 2.");

        let state = vm.store.get_working_set("default").unwrap();
        assert_eq!(
            state.invariants.get("arch"),
            Some(&"no unwrap in proxy".to_string())
        );

        // Replacing the same key keeps exactly one entry.
        execute_virtual_memory_tool(
            &vm,
            "cw_pin",
            &json!({"key": "arch", "rule": "updated rule"}),
        )
        .unwrap();
        let state = vm.store.get_working_set("default").unwrap();
        assert_eq!(state.invariants.len(), 1);
        assert_eq!(
            state.invariants.get("arch"),
            Some(&"updated rule".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn explicit_session_id_arg_is_respected() {
        let (root, vm) = fresh_context();
        execute_virtual_memory_tool(
            &vm,
            "cw_pin",
            &json!({"session_id": "sess-a", "key": "k", "rule": "r"}),
        )
        .unwrap();

        assert!(
            vm.store
                .get_working_set("default")
                .unwrap()
                .invariants
                .is_empty()
        );
        let sess_a = vm.store.get_working_set("sess-a").unwrap();
        assert_eq!(sess_a.invariants.get("k"), Some(&"r".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mount_refreshes_edited_file_before_slicing() {
        let (root, vm) = fresh_context();
        // Force the lazy index to build on the v1 content.
        execute_virtual_memory_tool(
            &vm,
            "cw_mount",
            &json!({"uri": "repo://src/lib.rs#target_fn"}),
        )
        .unwrap();

        // Mid-session edit: line shift + new declaration + changed body.
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn helper() -> u8 {\n    1\n}\n\npub fn target_fn() -> u32 {\n    41 + 1\n}\n",
        )
        .unwrap();

        let out = execute_virtual_memory_tool(
            &vm,
            "cw_mount",
            &json!({"uri": "repo://src/lib.rs#target_fn"}),
        )
        .unwrap();
        // target_fn moved from line 1 to line 5 in the edited file.
        assert!(out.contains("lines: 5-7"), "{out}");

        let state = vm.store.get_working_set("default").unwrap();
        assert_eq!(
            state.mounted[&SymbolUri("repo://src/lib.rs#target_fn".into())].content,
            "pub fn target_fn() -> u32 {\n    41 + 1\n}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_mode_and_missing_arguments_are_transport_errors() {
        let (root, vm) = fresh_context();
        assert!(
            execute_virtual_memory_tool(
                &vm,
                "cw_mount",
                &json!({"uri": "repo://src/lib.rs#target_fn", "mode": "bogus"})
            )
            .unwrap_err()
            .to_string()
            .contains("Invalid mode 'bogus'")
        );

        assert!(
            execute_virtual_memory_tool(&vm, "cw_mount", &json!({}))
                .unwrap_err()
                .to_string()
                .contains("uri is required")
        );
        assert!(
            execute_virtual_memory_tool(&vm, "cw_pin", &json!({"key": "k"}))
                .unwrap_err()
                .to_string()
                .contains("rule is required")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
