use serde_json::Value;

/// Infers project workspace root from file paths in tool arguments.
pub fn infer_workspace_from_tool_args(args: &Value) -> Option<String> {
    for key in ["Path", "path", "DirectoryPath", "SearchPath", "CommandLine"] {
        if let Some(val) = args.get(key).and_then(|v| v.as_str())
            && let Some(ws) = extract_project_root_from_path_str(val)
        {
            return Some(ws);
        }
    }
    None
}

/// Infers project workspace root from text mentions or file URIs.
pub fn infer_workspace_from_text(text: &str) -> Option<String> {
    if let Some(idx) = text.find("file:///") {
        let after = &text[idx + 7..];
        let path_part: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ')' && *c != ']' && *c != '"' && *c != '#')
            .collect();
        if let Some(ws) = extract_project_root_from_path_str(&path_part) {
            return Some(ws);
        }
    }
    extract_project_root_from_path_str(text)
}

/// Extracts `/home/<user>/<category>/<project>` or `/Users/...` project root from a string path.
pub fn extract_project_root_from_path_str(s: &str) -> Option<String> {
    for token in
        s.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '[' | ']'))
    {
        let clean = token.strip_prefix("file://").unwrap_or(token);
        if clean.starts_with("/home/") || clean.starts_with("/Users/") {
            if clean.contains("/.gemini/")
                || clean.contains("/.claude/")
                || clean.contains("/.local/")
            {
                continue;
            }
            let parts: Vec<&str> = clean.split('/').filter(|p| !p.is_empty()).collect();
            // e.g. ["home", "gionag", "Development", "contextWatcher", ...]
            if parts.len() >= 4
                && matches!(
                    parts[2],
                    "Development"
                        | "Projects"
                        | "repos"
                        | "work"
                        | "workspace"
                        | "code"
                        | "src"
                        | "git"
                )
            {
                return Some(format!(
                    "/{}/{}/{}/{}",
                    parts[0], parts[1], parts[2], parts[3]
                ));
            } else if parts.len() >= 3 {
                return Some(format!("/{}/{}/{}", parts[0], parts[1], parts[2]));
            }
        }
    }
    None
}
