import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

let cachedControlToken = null;
const transformedMessageSets = new WeakSet();
let lastTransformRecord = { sid: null, length: 0, timestamp: 0 };

/**
 * Retrieves the local ContextWatcher control token securely.
 * Purges CW_CONTROL_TOKEN and CW_CONTROL_TOKEN_FILE from process.env immediately
 * so model-invoked child processes (e.g. bash tools) cannot inherit them.
 */
function getControlToken() {
  if (cachedControlToken) return cachedControlToken;

  const envFile = process.env.CW_CONTROL_TOKEN_FILE;
  const tokenFile =
    envFile ||
    path.join(os.homedir(), ".local/share/context-watcher/control-token");

  try {
    const stat = fs.lstatSync(tokenFile);
    if (stat.isFile() && (process.platform === "win32" || (stat.mode & 0o077) === 0)) {
      const token = fs.readFileSync(tokenFile, "utf8").trim();
      if (token.length >= 32 && token.length <= 128 && /^[a-zA-Z0-9_-]+$/.test(token)) {
        cachedControlToken = token;
      }
    }
  } catch {
    // File missing or inaccessible
  }

  // Security: Purge sensitive credentials from process.env immediately after reading
  if (process.env.CW_CONTROL_TOKEN) {
    if (!cachedControlToken && /^[a-zA-Z0-9_-]{32,128}$/.test(process.env.CW_CONTROL_TOKEN)) {
      cachedControlToken = process.env.CW_CONTROL_TOKEN;
    }
    delete process.env.CW_CONTROL_TOKEN;
  }
  if (process.env.CW_CONTROL_TOKEN_FILE) {
    delete process.env.CW_CONTROL_TOKEN_FILE;
  }

  return cachedControlToken;
}

/**
 * Resolves the canonical ContextWatcher session ID.
 * Uses uniform 'omp_<sid>' prefixing across all hooks and slash commands without truncation.
 */
function getCanonicalSessionId(ctx, pi) {
  if (process.env.CW_SESSION_ID) {
    return process.env.CW_SESSION_ID.trim();
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

/**
 * Resolves and validates the ContextWatcher daemon URL.
 * Strictly enforces local loopback addresses (127.0.0.1, localhost, [::1]) to prevent token leakage.
 */
function getDaemonUrl() {
  const rawUrl =
    process.env.CW_DAEMON_URL ||
    `http://127.0.0.1:${process.env.CW_PORT || process.env.CW_DAEMON_PORT || 4400}`;

  try {
    const parsed = new URL(rawUrl);
    const host = parsed.hostname.toLowerCase();
    const isLoopback =
      host === "127.0.0.1" ||
      host === "localhost" ||
      host === "::1" ||
      host === "[::1]";

    if (!isLoopback || parsed.protocol !== "http:") {
      return "http://127.0.0.1:4400";
    }
    return `${parsed.protocol}//${parsed.host}`;
  } catch {
    return "http://127.0.0.1:4400";
  }
}

/**
 * Identifies the model provider for protocol validation and transformation.
 */
function resolveProvider(ctx, event) {
  const modelProvider = (
    ctx?.model?.provider ||
    ctx?.agent?.state?.model?.provider ||
    ""
  ).toLowerCase();

  if (modelProvider.includes("anthropic") || modelProvider.includes("claude")) {
    return "anthropic";
  }
  if (modelProvider.includes("gemini") || modelProvider.includes("google")) {
    return "gemini";
  }
  if (modelProvider.includes("deepseek")) {
    return "deepseek";
  }
  if (modelProvider.includes("ollama")) {
    return "ollama";
  }
  const payload = event?.payload;
  if (payload) {
    if (Array.isArray(payload.contents)) return "gemini";
    if (payload.system && Array.isArray(payload.messages)) return "anthropic";
  }
  return "openai";
}

/**
 * Submits in-memory messages to the local contextWatcher daemon for active mutation overlays.
 * Ensures fail-open safety on network errors, timeouts, or unparseable responses.
 */
async function transformMessages(messages, sid, provider) {
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
        body: JSON.stringify({ messages, provider: provider || "openai" }),
        signal: AbortSignal.timeout(1500),
      }
    );

    if (res.ok) {
      const data = await res.json();
      if (Array.isArray(data?.messages)) {
        return data.messages;
      }
    }
  } catch {
    // Fail-open: network offline, timeout, or daemon unready
  }
  return messages;
}

function runCwCli(args, extraEnv = {}) {
  const env = { ...process.env, CW_HARNESS: "omp", ...extraEnv };
  return new Promise((resolve, reject) => {
    execFile("cw", args, { timeout: 15000, env }, (error, stdout, stderr) => {
      if (error) {
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
  const sid = getCanonicalSessionId(ctx, pi);
  const extraEnv = sid ? { CW_SESSION_ID: sid } : {};

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
  // Pre-load and cleanse control token from process.env immediately on extension initialization
  getControlToken();

  // 1. Native Runtime Context Hook (CW-111 / CW-111B)
  // Intercepts in-memory messages before prompt formatting, applies active CW overlays,
  // leaving the on-disk session transcript 100% immutable and intact.
  pi.on?.("context", async (event, ctx) => {
    try {
      if (!Array.isArray(event?.messages)) return;
      const sid = getCanonicalSessionId(ctx, pi);
      const provider = resolveProvider(ctx, event);
      const transformed = await transformMessages(event.messages, sid, provider);

      if (transformed && transformed !== event.messages) {
        transformedMessageSets.add(transformed);
        lastTransformRecord = {
          sid,
          length: transformed.length,
          timestamp: Date.now(),
        };
      }
      return { messages: transformed };
    } catch {
      return { messages: event?.messages };
    }
  });

  // 2. Native Runtime Before Provider Request Hook (CW-111 / CW-111B)
  // Fallback handler for requests bypassing the context hook (e.g. direct payload dispatches).
  // Strictly skips transformation if context hook already processed the turn to prevent double-mutation.
  pi.on?.("before_provider_request", async (event, ctx) => {
    try {
      const messages = event?.payload?.messages;
      if (!Array.isArray(messages)) return event?.payload;

      const sid = getCanonicalSessionId(ctx, pi);

      // Single-pass idempotency check: If context hook already transformed this turn, pass through cleanly.
      const now = Date.now();
      const isAlreadyTransformed =
        transformedMessageSets.has(messages) ||
        (lastTransformRecord.sid === sid &&
          lastTransformRecord.length === messages.length &&
          now - lastTransformRecord.timestamp < 3000);

      if (isAlreadyTransformed) {
        return event.payload;
      }

      const provider = resolveProvider(ctx, event);
      const transformed = await transformMessages(messages, sid, provider);

      if (transformed && transformed !== messages) {
        transformedMessageSets.add(transformed);
        lastTransformRecord = {
          sid,
          length: transformed.length,
          timestamp: Date.now(),
        };
        return {
          ...event.payload,
          messages: transformed,
        };
      }
    } catch {
      return event?.payload;
    }
    return event?.payload;
  });

  // 3. Register canonical slash commands
  pi.registerCommand?.("cw", {
    description: "contextWatcher: /cw optimize [min|mid|max], /cw gain, /cw status",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });

  pi.registerCommand?.("context-watcher", {
    description: "contextWatcher: /context-watcher [optimize min|mid|max | gain | status]",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });
}
