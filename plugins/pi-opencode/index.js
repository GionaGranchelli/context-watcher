import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

function getControlToken() {
  const tokenFile =
    process.env.CW_CONTROL_TOKEN_FILE ||
    path.join(os.homedir(), ".local/share/context-watcher/control-token");
  try {
    const stat = fs.lstatSync(tokenFile);
    if (!stat.isFile()) return null;
    if (process.platform !== "win32" && (stat.mode & 0o077) !== 0) {
      return null;
    }
    const token = fs.readFileSync(tokenFile, "utf8").trim();
    if (token.length >= 32 && token.length <= 128 && /^[a-zA-Z0-9_-]+$/.test(token)) {
      return token;
    }
  } catch {
    return null;
  }
  return null;
}

function resolveSessionId(ctx, pi) {
  if (process.env.CW_SESSION_ID) {
    return process.env.CW_SESSION_ID;
  }
  const rawId =
    ctx?.sessionManager?.getSessionId?.() ||
    ctx?.session?.id ||
    ctx?.sessionId ||
    pi?.sessionId;
  if (!rawId) return null;
  const strId = String(rawId).trim();
  return strId.startsWith("omp_") ? strId : `omp_${strId}`;
}

function getDaemonUrl() {
  if (process.env.CW_DAEMON_URL) {
    return process.env.CW_DAEMON_URL.replace(/\/+$/, "");
  }
  const port = process.env.CW_PORT || process.env.CW_DAEMON_PORT || 4400;
  return `http://127.0.0.1:${port}`;
}

async function transformMessages(messages, sid) {
  if (!sid || !Array.isArray(messages) || messages.length === 0) {
    return messages;
  }
  const token = getControlToken();
  const daemonUrl = getDaemonUrl();
  try {
    const res = await fetch(
      `${daemonUrl}/api/v1/sessions/${encodeURIComponent(sid)}/transform`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(token ? { "x-cw-control-token": token } : {}),
        },
        body: JSON.stringify({ messages, provider: "openai" }),
        signal: AbortSignal.timeout(2000),
      }
    );
    if (res.ok) {
      const data = await res.json();
      if (Array.isArray(data?.messages)) {
        return data.messages;
      }
    }
  } catch {
    // Fail-open: network failure, daemon offline, timeout
  }
  return messages;
}

function runCwCli(args, extraEnv = {}) {
  const env = { ...process.env, CW_HARNESS: "omp", ...extraEnv };
  return new Promise((resolve, reject) => {
    execFile("cw", args, { timeout: 15000, env }, (error, stdout, stderr) => {
      if (error) {
        // Fallback to absolute ~/.local/bin/cw if cw not in ambient PATH
        const fallbackPath = `${process.env.HOME || "/tmp"}/.local/bin/cw`;
        execFile(fallbackPath, args, { timeout: 15000, env }, (err2, out2, errStr2) => {
          if (err2) return reject(new Error(errStr2 || err2.message));
          resolve(out2 || "");
        });
        return;
      }
      resolve(stdout || "");
    });
  });
}

const cwHandler = async (rawArgs, ctx, pi) => {
  const parts = String(rawArgs || "").trim().split(/\s+/).filter(Boolean);
  let action = parts[0] || "status";
  let param = parts[1] || "mid";
  const sessId = ctx?.session?.id || ctx?.sessionId || pi?.sessionId;
  const extraEnv = sessId ? { CW_SESSION_ID: `omp_${String(sessId).slice(0, 12)}` } : {};

  // Support direct `/context-watcher mid` or `/cw min` without requiring `optimize` keyword
  if (["min", "mid", "max", "clean_code_state", "surgical_log_diet", "max_token_diet"].includes(action)) {
    param = action;
    action = "optimize";
  }

  if (action === "gain") {
    try {
      ctx?.ui?.notify?.("Fetching contextWatcher token savings...", "info");
      const out = await runCwCli(["gain"], extraEnv);
      pi.sendMessage(`\`\`\`text\n${out.trim()}\n\`\`\``);
    } catch (err) {
      ctx?.ui?.notify?.(`cw gain error: ${err.message}`, "error");
    }
    return;
  }

  if (action === "status") {
    try {
      const out = await runCwCli(["status"], extraEnv);
      pi.sendMessage(`\`\`\`text\n${out.trim()}\n\`\`\``);
    } catch (err) {
      ctx?.ui?.notify?.(`cw status error: ${err.message}`, "error");
    }
    return;
  }

  if (action === "optimize" || action === "opt") {
    const level = ["min", "mid", "max", "clean_code_state", "surgical_log_diet", "max_token_diet"].includes(param)
      ? param
      : "mid";
    try {
      ctx?.ui?.notify?.(`Running contextWatcher optimize (${level})...`, "info");
      const out = await runCwCli(["optimize", "-s", level, "--sync-back"], extraEnv);
      pi.sendMessage(`\`\`\`text\n${out.trim()}\n\`\`\``);
      ctx?.ui?.notify?.(`Context compacted (${level}).`, "info");
    } catch (err) {
      ctx?.ui?.notify?.(`cw optimize error: ${err.message}`, "error");
    }
    return;
  }

  ctx?.ui?.notify?.("Usage: /context-watcher [optimize min|mid|max | gain | status] or /cw [min|mid|max]", "warning");
};

export default function contextWatcherExtension(pi) {
  // 1. Native Runtime Context Hook (CW-111)
  // Intercepts in-memory messages before prompt formatting, applies active CW overlays,
  // leaving the on-disk session transcript 100% immutable and intact.
  pi.on?.("context", async (event, ctx) => {
    try {
      if (!Array.isArray(event?.messages)) return;
      const sid = resolveSessionId(ctx, pi);
      const transformed = await transformMessages(event.messages, sid);
      return { messages: transformed };
    } catch {
      return { messages: event?.messages };
    }
  });

  // 2. Native Runtime Before Provider Request Hook (CW-111)
  // Ensures provider payload level prompt optimization if context was bypassed.
  pi.on?.("before_provider_request", async (event, ctx) => {
    try {
      if (!Array.isArray(event?.payload?.messages)) return event?.payload;
      const sid = resolveSessionId(ctx, pi);
      const transformed = await transformMessages(event.payload.messages, sid);
      return {
        ...event.payload,
        messages: transformed,
      };
    } catch {
      return event?.payload;
    }
  });

  // 3. Register slash commands
  pi.registerCommand?.("cw", {
    description: "contextWatcher: /cw optimize [min|mid|max], /cw gain, /cw status",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });

  pi.registerCommand?.("context-watcher", {
    description: "contextWatcher: /context-watcher [optimize min|mid|max | gain | status]",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });
}
