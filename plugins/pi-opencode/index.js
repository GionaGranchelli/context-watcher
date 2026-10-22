import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

function runCwCli(args) {
  return new Promise((resolve, reject) => {
    execFile("cw", args, { timeout: 15000 }, (error, stdout, stderr) => {
      if (error) {
        // Fallback to absolute ~/.local/bin/cw if cw not in ambient PATH
        const fallbackPath = `${process.env.HOME || "/tmp"}/.local/bin/cw`;
        execFile(fallbackPath, args, { timeout: 15000 }, (err2, out2, errStr2) => {
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

  // Support direct `/context-watcher mid` or `/cw min` without requiring `optimize` keyword
  if (["min", "mid", "max", "clean_code_state", "surgical_log_diet", "max_token_diet"].includes(action)) {
    param = action;
    action = "optimize";
  }

  if (action === "gain") {
    try {
      ctx?.ui?.notify?.("Fetching contextWatcher token savings...", "info");
      const out = await runCwCli(["gain"]);
      pi.sendMessage(`\`\`\`text\n${out.trim()}\n\`\`\``);
    } catch (err) {
      ctx?.ui?.notify?.(`cw gain error: ${err.message}`, "error");
    }
    return;
  }

  if (action === "status") {
    try {
      const out = await runCwCli(["status"]);
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
      const out = await runCwCli(["optimize", "-s", level, "--sync-back"]);
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
  // Register both /cw and /context-watcher for full compatibility
  pi.registerCommand("cw", {
    description: "contextWatcher: /cw optimize [min|mid|max], /cw gain, /cw status",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });

  pi.registerCommand("context-watcher", {
    description: "contextWatcher: /context-watcher [optimize min|mid|max | gain | status]",
    handler: (args, ctx) => cwHandler(args, ctx, pi),
  });
}
