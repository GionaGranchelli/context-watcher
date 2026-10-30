use anyhow::Result;
use std::path::{Path, PathBuf};

use super::daemon::{is_daemon_running, start_daemon_detached};

/// Registers the `contextwatcher` MCP server for Claude Code at user scope.
///
/// Claude Code reads user-scoped MCP servers from `~/.claude.json` (top-level `mcpServers`),
/// not from `~/.claude/settings.json`. Prefer the official `claude mcp add` CLI; if it is not
/// on PATH, merge the entry into `~/.claude.json` directly, keeping a backup.
fn register_claude_code_mcp(home_path: &Path, cw_cmd_str: &str) {
    let via_cli = std::process::Command::new("claude")
        .args(["mcp", "remove", "--scope", "user", "contextwatcher"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
        && std::process::Command::new("claude")
            .args([
                "mcp",
                "add",
                "--scope",
                "user",
                "contextwatcher",
                "--",
                cw_cmd_str,
                "mcp",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if via_cli {
        return;
    }

    let config_path = home_path.join(".claude.json");
    let mut root: serde_json::Value = match std::fs::read_to_string(&config_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            // Never overwrite a config we could not parse.
            Err(_) => return,
        },
        Err(_) => serde_json::json!({}),
    };
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return;
    };
    servers.insert(
        "contextwatcher".to_string(),
        serde_json::json!({ "type": "stdio", "command": cw_cmd_str, "args": ["mcp"], "env": {} }),
    );
    if config_path.exists() {
        let _ = std::fs::copy(&config_path, home_path.join(".claude.json.cw.bak"));
    }
    if let Ok(pretty) = serde_json::to_string_pretty(&root) {
        let tmp = home_path.join(".claude.json.cw.tmp");
        if std::fs::write(&tmp, format!("{pretty}\n")).is_ok() {
            let _ = std::fs::rename(&tmp, &config_path);
        }
    }
}

pub(crate) fn parse_json_string_array(s: &str) -> Option<Vec<String>> {
    let trimmed = s.trim();
    if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
        return Some(arr);
    }
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    if start < end {
        let slice = &s[start..=end];
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(slice) {
            return Some(arr);
        }
    }
    None
}

/// Merges contextWatcher variables into Hermes terminal.client_env_passthrough without clobbering existing configuration.
fn merge_hermes_client_env_passthrough() -> Result<()> {
    const REQUIRED_VARS: &[&str] = &[
        "CW_HERMES_OVERRIDE_FILE",
        "CW_SESSION_ID",
        "CW_CONTROL_TOKEN_FILE",
        "CW_WORKSPACE",
    ];

    let output = std::process::Command::new("hermes")
        .args(["config", "get", "--json", "terminal.client_env_passthrough"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Distinguish explicitly absent setting from failed/unrecognized output.
    // Preserve existing configuration on every error path; never overwrite settings when their prior value is unknown.
    let existing_vars: Vec<String> = if !output.status.success() {
        if stdout.contains("Config key not set") || stderr.contains("Config key not set") {
            Vec::new()
        } else {
            anyhow::bail!(
                "Failed to query Hermes terminal.client_env_passthrough (status {:?}): {stderr} {stdout}",
                output.status.code()
            );
        }
    } else {
        parse_json_string_array(&stdout).ok_or_else(|| {
            anyhow::anyhow!("Unrecognized format returned by hermes config get --json: {stdout}")
        })?
    };

    let mut updated = existing_vars.clone();
    let mut changed = false;
    for var in REQUIRED_VARS {
        if !updated.iter().any(|v| v == *var) {
            updated.push(var.to_string());
            changed = true;
        }
    }

    if changed {
        let json_val = serde_json::to_string(&updated)?;
        let status = std::process::Command::new("hermes")
            .args([
                "config",
                "set",
                "terminal.client_env_passthrough",
                &json_val,
            ])
            .status()?;
        if !status.success() {
            anyhow::bail!("hermes config set returned non-zero exit code");
        }
    }

    Ok(())
}

/// Installs contextWatcher integrations across all supported harnesses (Antigravity, Codex, Claude Code, Hermes, Pi/OMP, OpenCode).
pub fn run_init_installer(dry_run: bool, auto_start: bool) -> Result<()> {
    let home_path = cw_core::home_dir();
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cw"));
    let local_bin_dir = home_path.join(".local/bin");
    let installed_cw_bin = local_bin_dir.join("cw");

    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!(
        "│ contextWatcher (`cw init`) — Multi-Harness Plugin & Hook Installer {}│",
        if dry_run { "(DRY RUN)" } else { "         " }
    );
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    // 1. Install `cw` binary into ~/.local/bin/cw
    if !dry_run {
        let _ = std::fs::create_dir_all(&local_bin_dir);
        if current_exe != installed_cw_bin && current_exe.exists() {
            let tmp_bin = local_bin_dir.join("cw.tmp");
            if std::fs::copy(&current_exe, &tmp_bin).is_ok() {
                let _ = std::fs::rename(&tmp_bin, &installed_cw_bin);
            }
        }
    }
    let cw_cmd_str = if installed_cw_bin.exists() || !dry_run {
        installed_cw_bin.to_string_lossy().to_string()
    } else {
        current_exe.to_string_lossy().to_string()
    };
    println!("  [1/5] Global CLI Binary      : {}", cw_cmd_str);

    // 2. Antigravity Plugin (~/.gemini/config/plugins/contextwatcher/)
    let agy_plugin_dir = home_path.join(".gemini/config/plugins/contextwatcher");
    let agy_skill_dir = agy_plugin_dir.join("skills/context-watcher");
    let agy_skill_cw_dir = agy_plugin_dir.join("skills/cw");
    let agy_global_skill_dir = home_path.join(".gemini/config/skills/context-watcher");
    let agy_global_skill_cw_dir = home_path.join(".gemini/config/skills/cw");
    let agy_commands_dir = agy_plugin_dir.join("commands");
    let skill_md = r#"---
name: context-watcher
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

When invoked for `/context-watcher` or `/cw` with any subcommand or level (`min`, `mid`, `max`, `status`, `gain`):

**Execute the exact command directly using the shell/bash tool immediately on turn 1. Do not perform preliminary environment checks, do not search for config files, and do not run `which cw` or `--help`.**

### Direct Shell Commands:
- **Optimization:**
  - `min`: `~/.local/bin/cw optimize -s min --sync-back` (0-KV-break uncached duplicate reads/tail bloat + rule reminders; use `clean_code_state` for full covered read deduplication)
  - `mid`: `~/.local/bin/cw optimize -s mid --sync-back` (compacts old tool/command outputs and unsigned thinking blocks)
  - `max`: `~/.local/bin/cw optimize -s max --sync-back` (combines covered read deduplication, old tool/thinking compaction, and rule reminders; use `cw_fold_turns` with an explicit summary to fold turns)
- **Inspection:**
  - `status`: `~/.local/bin/cw status`
  - `gain`: `~/.local/bin/cw gain`

If the `cw_optimize_strategy` MCP tool is directly available in your active tool list, you may call it with `{"strategy_id": "<level>", "export_compacted": true}` (which applies the session overlay and exports a separate compacted `.jsonl` copy without modifying the source transcript in-place). Otherwise, execute the shell command above directly.
"#;

    let skill_cw_md = r#"---
name: cw
description: Inspects and compacts the active LLM context window, KV cache prefix, stale file reads, and noisy tool outputs via contextWatcher (`cw`). Use when context exceeds 80k tokens, health score drops, or the user asks to optimize/compact context.
---

# contextWatcher (`cw`) — Context & KV-Cache Optimization Skill

When invoked for `/cw` or `/context-watcher` with any subcommand or level (`min`, `mid`, `max`, `status`, `gain`):

**Execute the exact command directly using the shell/bash tool immediately on turn 1. Do not perform preliminary environment checks, do not search for config files, and do not run `which cw` or `--help`.**

### Direct Shell Commands:
- **Optimization:**
  - `min`: `~/.local/bin/cw optimize -s min --sync-back` (0-KV-break uncached duplicate reads/tail bloat + rule reminders; use `clean_code_state` for full covered read deduplication)
  - `mid`: `~/.local/bin/cw optimize -s mid --sync-back` (compacts old tool/command outputs and unsigned thinking blocks)
  - `max`: `~/.local/bin/cw optimize -s max --sync-back` (combines covered read deduplication, old tool/thinking compaction, and rule reminders; use `cw_fold_turns` with an explicit summary to fold turns)
- **Inspection:**
  - `status`: `~/.local/bin/cw status`
  - `gain`: `~/.local/bin/cw gain`

If the `cw_optimize_strategy` MCP tool is directly available in your active tool list, you may call it with `{"strategy_id": "<level>", "export_compacted": true}` (which applies the session overlay and exports a separate compacted `.jsonl` copy without modifying the source transcript in-place). Otherwise, execute the shell command above directly.
"#;

    let agy_mcp_json = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "contextwatcher": {
                "command": cw_cmd_str,
                "args": ["mcp"],
                "cwd": "",
                "env": null
            }
        }
    }))?;

    if !dry_run {
        let _ = std::fs::create_dir_all(&agy_skill_dir);
        let _ = std::fs::create_dir_all(&agy_skill_cw_dir);
        let _ = std::fs::create_dir_all(&agy_global_skill_dir);
        let _ = std::fs::create_dir_all(&agy_global_skill_cw_dir);
        let _ = std::fs::create_dir_all(&agy_commands_dir);
        let _ = std::fs::write(
            agy_plugin_dir.join("plugin.json"),
            "{\n  \"name\": \"contextwatcher\"\n}\n",
        );
        let _ = std::fs::write(agy_plugin_dir.join("mcp_config.json"), &agy_mcp_json);
        let _ = std::fs::write(agy_skill_dir.join("SKILL.md"), skill_md);
        let _ = std::fs::write(agy_skill_cw_dir.join("SKILL.md"), skill_cw_md);
        let _ = std::fs::write(agy_global_skill_dir.join("SKILL.md"), skill_md);
        let _ = std::fs::write(agy_global_skill_cw_dir.join("SKILL.md"), skill_cw_md);
        let _ = std::fs::write(
            agy_commands_dir.join("cw.toml"),
            "description = \"Run contextWatcher compaction: /cw optimize min|mid|max, /cw gain, or /cw status\"\n\
             prompt = \"Execute contextWatcher optimization with intensity {{args}} (min, mid, or max). Call tool cw_optimize_strategy with strategy_id: '{{args}}' and export_compacted: true to apply session overlays and export a compacted copy.\"\n",
        );
        let _ = std::fs::write(
            agy_commands_dir.join("context-watcher.toml"),
            "description = \"Run contextWatcher compaction: /context-watcher optimize min|mid|max, gain, or status\"\n\
             prompt = \"Execute contextWatcher optimization with intensity {{args}} (min, mid, or max). Call tool cw_optimize_strategy with strategy_id: '{{args}}' and export_compacted: true to apply session overlays and export a compacted copy.\"\n",
        );
        let _ = std::fs::write(
            agy_commands_dir.join("cw-gain.toml"),
            "description = \"Show contextWatcher token and dollar ($) savings scoreboard\"\n\
             prompt = \"Run the shell command `cw gain` using run_command and display the savings scoreboard.\"\n",
        );
    }
    println!(
        "  [2/6] Antigravity Plugin     : {} (MCP + /context-watcher & /cw slash commands)",
        agy_plugin_dir.display()
    );

    // 3. Codex CLI (~/.codex/config.toml + ~/.codex/CW.md + ~/.codex/skills/context-watcher/ & skills/cw/)
    let codex_dir = home_path.join(".codex");
    if codex_dir.exists() {
        let codex_cfg = codex_dir.join("config.toml");
        let codex_cw_md = codex_dir.join("CW.md");
        let codex_agents_md = codex_dir.join("AGENTS.md");
        let codex_skill_cw_dir = codex_dir.join("skills/cw");
        let codex_skill_full_dir = codex_dir.join("skills/context-watcher");

        let cw_md_content = format!(
            "# contextWatcher (`cw`) — Context & KV-Cache Optimizer\n\n\
             `cw` monitors and compacts context bloat (stale file reads, old tool outputs, reasoning blocks) across turns.\n\n\
             ## Commands & MCP Tools\n\n\
             - Slash commands: `/context-watcher optimize min|mid|max` or `/cw optimize min|mid|max`, `/cw gain`, `/cw status`\n\
             - MCP tools: `cw_context_status`, `cw_optimize_strategy`, `cw_evict_stale_reads`, `cw_fold_turns`, `cw_pin_context`\n\
             - CLI commands: `{cw_cmd_str} gain`, `{cw_cmd_str} status`, `{cw_cmd_str} optimize min|mid|max`\n"
        );

        if !dry_run {
            let _ = std::fs::create_dir_all(&codex_skill_cw_dir);
            let _ = std::fs::create_dir_all(&codex_skill_full_dir);
            let _ = std::fs::write(&codex_cw_md, cw_md_content);
            let _ = std::fs::write(codex_skill_cw_dir.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(codex_skill_full_dir.join("SKILL.md"), skill_md);

            // Also install into ~/.agents/skills if the folder exists (scanned by Codex)
            let shared_agents_skills = home_path.join(".agents/skills");
            if shared_agents_skills.exists() {
                let _ = std::fs::create_dir_all(shared_agents_skills.join("cw"));
                let _ = std::fs::create_dir_all(shared_agents_skills.join("context-watcher"));
                let _ = std::fs::write(shared_agents_skills.join("cw/SKILL.md"), skill_cw_md);
                let _ = std::fs::write(
                    shared_agents_skills.join("context-watcher/SKILL.md"),
                    skill_md,
                );
            }
            if codex_cfg.exists()
                && let Ok(existing) = std::fs::read_to_string(&codex_cfg)
                && !existing.contains("[mcp_servers.contextwatcher]")
            {
                let _ = std::fs::copy(&codex_cfg, codex_dir.join("config.toml.cw.bak"));
                let appended = format!(
                    "{existing}\n\n[mcp_servers.contextwatcher]\ncommand = \"{cw_cmd_str}\"\nargs = [\"mcp\"]\n"
                );
                let _ = std::fs::write(&codex_cfg, appended);
            }
            if codex_agents_md.exists()
                && let Ok(existing_agents) = std::fs::read_to_string(&codex_agents_md)
            {
                let ref_line = format!("@{}", codex_cw_md.display());
                if !existing_agents.contains(&ref_line) {
                    let updated = format!("{}\n{}\n", existing_agents.trim_end(), ref_line);
                    let _ = std::fs::write(&codex_agents_md, updated);
                }
            }
        }
        println!(
            "  [3/6] Codex CLI Integration  : {} ([mcp_servers.contextwatcher] + /context-watcher & /cw skills)",
            codex_cfg.display()
        );
    }

    // 4. Claude Code (~/.claude/settings.json + commands/context-watcher.md & cw.md)
    let claude_dir = home_path.join(".claude");
    if claude_dir.exists() {
        let settings_path = claude_dir.join("settings.json");
        let commands_dir = claude_dir.join("commands");
        let skills_full_dir = claude_dir.join("skills/context-watcher");
        let skills_cw_dir = claude_dir.join("skills/cw");

        if !dry_run {
            let _ = std::fs::create_dir_all(&commands_dir);
            let _ = std::fs::create_dir_all(&skills_full_dir);
            let _ = std::fs::create_dir_all(&skills_cw_dir);

            let claude_cmd_content = "# /cw [optimize min|mid|max | gain | status]\n\n\
                 Execute contextWatcher optimization with arguments: `$ARGUMENTS`.\n\
                 - If `$ARGUMENTS` starts with `optimize` or `opt` or is `min`, `mid`, or `max`: call tool `cw_optimize_strategy` with `strategy_id` set to the level (defaulting to `\"mid\"`) and `export_compacted: true`.\n\
                 - If `$ARGUMENTS` is `gain`: run `cw gain` via Bash.\n\
                 - If `$ARGUMENTS` is `status`: call tool `cw_context_status`.\n";

            let _ = std::fs::write(commands_dir.join("cw.md"), claude_cmd_content);
            let _ = std::fs::write(commands_dir.join("context-watcher.md"), claude_cmd_content);
            let _ = std::fs::write(skills_full_dir.join("SKILL.md"), skill_md);
            let _ = std::fs::write(skills_cw_dir.join("SKILL.md"), skill_cw_md);

            register_claude_code_mcp(&home_path, &cw_cmd_str);

            // Earlier versions wrote `mcpServers.contextwatcher` into settings.json, which
            // Claude Code does not read MCP servers from. Remove that stale entry if present.
            if settings_path.exists()
                && let Some(mut root) = std::fs::read_to_string(&settings_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            {
                let mut changed = false;
                if let Some(obj) = root.as_object_mut() {
                    let should_remove_mcps = if let Some(mcp) =
                        obj.get_mut("mcpServers").and_then(|v| v.as_object_mut())
                    {
                        changed = mcp.remove("contextwatcher").is_some();
                        mcp.is_empty()
                    } else {
                        false
                    };
                    if should_remove_mcps {
                        obj.remove("mcpServers");
                    }
                }
                if changed {
                    let _ = std::fs::copy(&settings_path, claude_dir.join("settings.json.cw.bak"));
                    if let Ok(pretty) = serde_json::to_string_pretty(&root) {
                        let _ = std::fs::write(&settings_path, format!("{pretty}\n"));
                    }
                }
            }
        }
        println!(
            "  [4/6] Claude Code Config     : {} (user-scope MCP server `contextwatcher` + /context-watcher & /cw slash commands)",
            home_path.join(".claude.json").display()
        );
    }

    // 5. Hermes Agent (~/.hermes/skills/context-watcher/ & skills/cw/)
    let hermes_dir = home_path.join(".hermes");
    if hermes_dir.exists() {
        let hermes_skill_cw_dir = hermes_dir.join("skills/cw");
        let hermes_skill_full_dir = hermes_dir.join("skills/context-watcher");
        let hermes_plugin_dir = hermes_dir.join("plugins/context-watcher");
        if !dry_run {
            let _ = std::fs::create_dir_all(&hermes_skill_cw_dir);
            let _ = std::fs::create_dir_all(&hermes_skill_full_dir);
            let _ = std::fs::write(hermes_skill_cw_dir.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(hermes_skill_full_dir.join("SKILL.md"), skill_md);
            std::fs::create_dir_all(&hermes_plugin_dir)?;
            std::fs::write(
                hermes_plugin_dir.join("plugin.yaml"),
                include_str!("../../../../plugins/hermes/context-watcher/plugin.yaml"),
            )?;
            std::fs::write(
                hermes_plugin_dir.join("__init__.py"),
                include_str!("../../../../plugins/hermes/context-watcher/__init__.py"),
            )?;
            if let Err(err) = merge_hermes_client_env_passthrough() {
                eprintln!(
                    "           Warning: could not merge Hermes client_env_passthrough: {err}"
                );
            }
            let plugin_enabled = std::process::Command::new("hermes")
                .args([
                    "plugins",
                    "enable",
                    "context-watcher",
                    "--no-allow-tool-override",
                ])
                .status()
                .is_ok_and(|status| status.success());
            if plugin_enabled {
                println!("           Hermes contextWatcher plugin enabled for new sessions.");
                println!(
                    "           Note: To apply changes to an active gateway service, restart it when convenient:"
                );
                println!("                 systemctl --user restart hermes-gateway");
            } else {
                println!(
                    "           Run `hermes plugins enable context-watcher` to activate proxy middleware."
                );
            }
            println!(
                "           Launch with `cw wrap hermes chat` (resume with `cw wrap hermes chat --resume <id>`); direct `hermes` calls bypass the cw proxy."
            );
        }
        println!(
            "  [5/6] Hermes Skill + Plugin  : {} (/context-watcher & /cw; enable proxy middleware)",
            hermes_skill_full_dir.display()
        );
    }

    // 6. Native Pi & OMP Agent (~/.pi/agent/settings.json + ~/.omp/agent/skills/)
    let pi_agent_dir = home_path.join(".pi/agent");
    let omp_agent_dir = home_path.join(".omp/agent");
    if pi_agent_dir.exists() || omp_agent_dir.exists() {
        let pi_ext_dir = home_path.join("Development/contextWatcher/plugins/pi-opencode");
        if !dry_run && pi_ext_dir.exists() {
            let _ = std::process::Command::new("pi")
                .args(["install", &pi_ext_dir.to_string_lossy()])
                .status();
            let _ = std::process::Command::new("omp")
                .args(["plugin", "install", &pi_ext_dir.to_string_lossy()])
                .status();
        }
        if !dry_run && omp_agent_dir.exists() {
            let omp_skill_cw = omp_agent_dir.join("skills/cw");
            let omp_skill_full = omp_agent_dir.join("skills/context-watcher");
            let _ = std::fs::create_dir_all(&omp_skill_cw);
            let _ = std::fs::create_dir_all(&omp_skill_full);
            let _ = std::fs::write(omp_skill_cw.join("SKILL.md"), skill_cw_md);
            let _ = std::fs::write(omp_skill_full.join("SKILL.md"), skill_md);
        }
        println!(
            "  [6/6] Native Pi & OMP Agent  : {} (Native /context-watcher & /cw commands registered)",
            pi_ext_dir.display()
        );
    }

    // 7. OpenCode (~/.config/opencode/config.json or ~/.opencode/config.json)
    let opencode_cfg_dir = home_path.join(".config/opencode");
    let opencode_alt_dir = home_path.join(".opencode");
    let opencode_data_dir = home_path.join(".local/share/opencode");
    if opencode_cfg_dir.exists() || opencode_alt_dir.exists() || opencode_data_dir.exists() {
        let target_cfg = if opencode_alt_dir.exists() {
            opencode_alt_dir.join("config.json")
        } else {
            opencode_cfg_dir.join("config.json")
        };
        if !dry_run {
            if let Some(parent) = target_cfg.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut cfg_val: serde_json::Value = std::fs::read_to_string(&target_cfg)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            if !cfg_val.is_object() {
                cfg_val = serde_json::json!({});
            }
            cfg_val["mcp"]["contextWatcher"] = serde_json::json!({
                "type": "local",
                "command": ["cw", "mcp", "--daemon-url", "http://127.0.0.1:4400"],
                "enabled": true
            });
            let _ = std::fs::write(&target_cfg, serde_json::to_string_pretty(&cfg_val).unwrap());
        }
        println!(
            "  [7/7] OpenCode Integration   : {} (MCP server registered)",
            target_cfg.display()
        );
    }

    // 5. Systemd User Service (~/.config/systemd/user/contextwatcher.service)
    let systemd_dir = home_path.join(".config/systemd/user");
    let service_path = systemd_dir.join("contextwatcher.service");
    let service_unit = format!(
        "[Unit]\n\
         Description=contextWatcher Daemon (Web UI :4400 + LLM Proxy :4401 + Transcript Watcher)\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart={cw_cmd_str} serve --port 4400 --proxy-port 4401\n\
         Restart=on-failure\n\
         RestartSec=3\n\n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    if !dry_run {
        let _ = std::fs::create_dir_all(&systemd_dir);
        let _ = std::fs::write(&service_path, service_unit);
        if auto_start {
            let mut started = false;
            let has_systemctl = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if has_systemctl {
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "enable", "--now", "contextwatcher.service"])
                    .status();
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "restart", "contextwatcher.service"])
                    .status();
                std::thread::sleep(std::time::Duration::from_millis(200));
                if is_daemon_running(4400) {
                    started = true;
                }
            }
            if !started && !is_daemon_running(4400) {
                let _ = start_daemon_detached(4400, 4401, None);
            }
        }
    }
    println!(
        "  [5/5] Systemd User Service   : {}",
        service_path.display()
    );

    println!("\n✅ contextWatcher integration complete! Verify anytime with:");
    println!(
        "   cw gain              # Show token & dollar ($) savings across all agents & projects"
    );
    println!("   cw status            # Inspect latest active session health & KV cache hit ratio");
    println!("   cw optimize          # Run or preview the 5 compaction strategies");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_string_array_handles_multiline_and_rejects_malformed() {
        let direct = r#"["VAR1", "VAR2"]"#;
        assert_eq!(
            parse_json_string_array(direct),
            Some(vec!["VAR1".to_string(), "VAR2".to_string()])
        );

        let multiline_with_prefix =
            "  Bitwarden Secrets Manager: applied 9 secrets\n[\n  \"A\",\n  \"B\"\n]\n";
        assert_eq!(
            parse_json_string_array(multiline_with_prefix),
            Some(vec!["A".to_string(), "B".to_string()])
        );

        let malformed = "  Bitwarden Secrets Manager: applied 9 secrets\nError: database locked\n";
        assert_eq!(parse_json_string_array(malformed), None);

        let not_array = "{\"error\": \"not an array\"}";
        assert_eq!(parse_json_string_array(not_array), None);
    }
}
