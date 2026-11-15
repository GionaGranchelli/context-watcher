//! Deterministic workspace symbol indexing (Milestone A).
//!
//! `SymbolIndex` maps stable `repo://<rel_path>` / `repo://<rel_path>#<symbol>` URIs to
//! exact byte and line ranges. All iteration is over a `BTreeMap`, so rendering,
//! resolution fallbacks, and build output are fully deterministic regardless of
//! filesystem enumeration order.

use std::collections::BTreeMap;
use std::fs::{self, DirEntry};
use std::path::Path;

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

/// Source extensions indexed for symbol extraction by [`SymbolIndex::build`].
const SOURCE_EXTENSIONS: &[&str] = &["rs", "kt", "ts", "tsx", "js", "py"];

/// Directory names excluded from workspace indexing (build artifacts and deps).
/// Hidden entries (leading `.`) are always skipped as well.
const EXCLUDED_DIRS: &[&str] = &["target", "node_modules", "dist", "build"];

/// Leading declaration modifiers that signature normalization strips, in a loop, so
/// combinations like `pub async fn` or `export default class` normalize cleanly.
/// Longer visibility forms must precede their shorter prefixes.
const DECL_MODIFIERS: &[&str] = &[
    "pub(crate) ",
    "pub(super) ",
    "pub ",
    "export ",
    "internal ",
    "open ",
    "abstract ",
    "sealed ",
    "data ",
    "final ",
    "static ",
    "async ",
    "unsafe ",
    "default ",
];

/// Extract top-level symbols from one source file.
///
/// Always emits a whole-file [`SymbolKind::File`] entry; for `.rs`, `.kt`, `.ts`,
/// `.tsx`, `.js` and `.py` files it additionally extracts top-level declarations
/// with exact byte/line ranges such that `source[entry.start_byte..entry.end_byte]`
/// is valid UTF-8 equal to the declaration block. Extraction is a deterministic
/// single pass: brace languages track `{`/`}` depth outside strings and comments,
/// Python tracks indentation (top level = zero indent). Nested declarations are
/// part of their parent's byte range and never emitted separately. Multi-line
/// headers contribute only their first physical line to the signature; a Rust
/// `mod name;` statement ends at its semicolon when no brace is opened.
pub fn extract_symbols_from_source(rel_path: &str, source: &str) -> Vec<SymbolEntry> {
    let file_entry = SymbolEntry {
        uri: SymbolUri(format!("repo://{rel_path}")),
        rel_path: rel_path.to_string(),
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
        kind: SymbolKind::File,
        signature: rel_path.to_string(),
    };
    let mut entries = vec![file_entry];
    if source.is_empty() {
        return entries;
    }

    let extension = match rel_path.rsplit_once('.') {
        Some((_, ext)) => ext,
        None => return entries,
    };
    if !SOURCE_EXTENSIONS.contains(&extension) {
        return entries;
    }

    let lines = line_ranges(source);
    if extension == "py" {
        extract_python(rel_path, source, &lines, &mut entries);
    } else {
        extract_brace_lang(
            rel_path,
            source,
            &lines,
            keywords_for(extension),
            &mut entries,
        );
    }
    entries
}

/// (start_inclusive, end_exclusive) byte ranges of every line; `end` excludes the
/// trailing `\n`. For non-empty sources the count matches `source.lines().count()`.
fn line_ranges(source: &str) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for chunk in source.split_inclusive('\n') {
        let end = start + chunk.len() - (chunk.ends_with('\n') as usize);
        lines.push((start, end));
        start += chunk.len();
    }
    lines
}

fn keywords_for(extension: &str) -> &'static [(&'static str, SymbolKind)] {
    match extension {
        "rs" => &[
            ("fn", SymbolKind::Function),
            ("struct", SymbolKind::Struct),
            ("enum", SymbolKind::Enum),
            ("trait", SymbolKind::Trait),
            ("impl", SymbolKind::Impl),
            ("mod", SymbolKind::Module),
        ],
        "kt" => &[
            ("class", SymbolKind::Class),
            ("fun", SymbolKind::Function),
            ("interface", SymbolKind::Trait),
        ],
        // TS/TSX share the JS keyword set.
        _ => &[
            ("class", SymbolKind::Class),
            ("function", SymbolKind::Function),
            ("interface", SymbolKind::Trait),
        ],
    }
}

/// Scanner state carried across lines for brace languages: current brace depth
/// (clamped at zero) and whether we are inside a `/* */` block comment. String
/// literals do not persist across lines (line-local approximation; raw string
/// delimiters such as Rust `r#"..."#` are a documented limitation).
struct BraceScanState {
    depth: usize,
    in_block_comment: bool,
}

/// Scan one line of brace-language source updating [`BraceScanState`], invoking
/// `on_structural(c, abs)` for each `{`, `}` or statement-terminating `;` found
/// outside strings and comments, where `abs` is the absolute byte offset of that
/// character within the whole file.
fn scan_brace_line(
    line: &str,
    state: &mut BraceScanState,
    line_start: usize,
    mut on_structural: impl FnMut(char, usize),
) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if state.in_block_comment {
            if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                state.in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        let c = line[i..].chars().next().unwrap();
        match c {
            '/' if bytes.get(i + 1) == Some(&b'/') => return, // rest of the line is a comment
            '/' if bytes.get(i + 1) == Some(&b'*') => {
                state.in_block_comment = true;
                i += 2;
            }
            q @ ('"' | '\'' | '`') => {
                i += 1;
                while i < line.len() {
                    let ch = line[i..].chars().next().unwrap();
                    if ch == '\\' {
                        i += 1 + line.get(i + 1..)
                            .and_then(|rest| rest.chars().next())
                            .map(char::len_utf8)
                            .unwrap_or(0);
                    } else if ch == q {
                        break;
                    } else {
                        i += ch.len_utf8();
                    }
                }
                // Consume the closing quote when present; otherwise string state
                // simply ends at end-of-line (line-local approximation). All three
                // supported quotes are ASCII, so a byte compare is exact.
                if i < line.len() && line.as_bytes()[i] == q as u8 {
                    i += 1;
                }
            }
            '{' => {
                state.depth += 1;
                on_structural(c, line_start + i);
                i += 1;
            }
            '}' => {
                if state.depth > 0 {
                    state.depth -= 1;
                }
                on_structural(c, line_start + i);
                i += 1;
            }
            ';' => {
                on_structural(c, line_start + i);
                i += 1;
            }
            _ => i += c.len_utf8(),
        }
    }
}

/// A declaration whose closing boundary has not been found yet.
struct PendingDecl {
    start_byte: usize,
    start_line: usize, // 1-indexed
    kind: SymbolKind,
    name: String,
    saw_brace: bool,
}

/// Scan a line while a declaration is pending: report the byte offset just past
/// its closing `}` (once a brace was opened and depth returns to zero) or `;`
/// (only when no brace has been seen yet), plus whether an opening brace appeared.
fn scan_for_close(
    line: &str,
    state: &mut BraceScanState,
    line_start: usize,
    saw_brace_at_line_start: bool,
) -> (Option<usize>, bool) {
    // Collect structural events first so the closure does not borrow `state`
    // while it is also updated through the same call.
    // The depth at the start of this line drives the replay below.
    let start_depth = state.depth;
    let mut events = Vec::new();
    scan_brace_line(line, state, line_start, |c, abs| events.push((c, abs)));

    let mut depth = start_depth;
    let mut saw_brace = saw_brace_at_line_start;
    let mut close_at = None;
    for &(c, abs) in &events {
        match c {
            '{' => {
                saw_brace = true;
                depth += 1;
            }
            '}' if depth > 0 => depth -= 1,
            _ => {}
        }
        if close_at.is_none() {
            let closed = match c {
                '}' => saw_brace && depth == 0,
                ';' => !saw_brace,
                _ => false,
            };
            if closed {
                close_at = Some(abs + 1);
            }
        }
    }
    (close_at, saw_brace)
}

/// Match a declaration start at the beginning of a line (after leading whitespace):
/// optional modifiers from [`DECL_MODIFIERS`], then one of `keywords`, followed by
/// whitespace or a JS/TS generator star. Returns `(kind, name)`.
fn match_decl_start(line: &str, keywords: &[(&str, SymbolKind)]) -> Option<(SymbolKind, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty()
        || trimmed.starts_with("//") // line / doc comment
        || trimmed.starts_with('#') // Rust attribute or Python-style marker
        || trimmed.starts_with('!')
    {
        return None;
    }

    let mut rest = trimmed;
    while let Some(stripped) = DECL_MODIFIERS.iter().find_map(|m| rest.strip_prefix(*m)) {
        rest = stripped.trim_start();
    }

    for (keyword, kind) in keywords {
        let Some(after_kw) = rest.strip_prefix(keyword) else {
            continue;
        };
        // Identifier boundary: keyword must end at EOL or before a space/tab/star.
        if !matches!(after_kw.chars().next(), None | Some(' ' | '\t' | '*')) {
            continue; // e.g. `fnx` is not an `fn` declaration start.
        }
        if let Some(name) = identifier_after(after_kw) {
            return Some((*kind, name));
        }
    }
    None
}

/// First identifier in `s`, skipping whitespace and JS/TS generator stars.
fn identifier_after(s: &str) -> Option<String> {
    let s = s.trim_start();
    let s = s.strip_prefix('*').unwrap_or(s).trim_start();
    let mut chars = s.chars().peekable();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    let mut name = String::new();
    name.push(first);
    while let Some(c) = chars.peek() {
        if c.is_ascii_alphanumeric() || *c == '_' {
            name.push(*c);
            chars.next();
        } else {
            break;
        }
    }
    Some(name)
}

/// Normalize a declaration header into its single-line signature: strip leading
/// modifiers (loop), then one trailing semicolon.
fn normalize_signature(header_cut: &str) -> String {
    let mut sig = header_cut.trim();
    while let Some(stripped) = DECL_MODIFIERS.iter().find_map(|m| sig.strip_prefix(*m)) {
        sig = stripped.trim_start();
    }
    sig = sig.strip_suffix(';').unwrap_or(sig);
    sig.to_string()
}

/// 1-indexed line number containing byte offset `pos`; a position on the trailing
/// newline boundary of a line counts as that same line.
fn line_number_of(lines: &[(usize, usize)], pos: usize) -> usize {
    for (n, &(start, end)) in lines.iter().enumerate() {
        if start <= pos && pos <= end {
            return n + 1;
        }
    }
    lines.len().max(1)
}

/// Single-pass top-level declaration extraction for brace languages.
fn extract_brace_lang(
    rel_path: &str,
    source: &str,
    lines: &[(usize, usize)],
    keywords: &[(&str, SymbolKind)],
    entries: &mut Vec<SymbolEntry>,
) {
    let mut state = BraceScanState {
        depth: 0,
        in_block_comment: false,
    };
    let mut pending: Option<PendingDecl> = None;

    for (idx, &(line_start, line_end)) in lines.iter().enumerate() {
        let line = &source[line_start..line_end];

        if let Some(decl) = pending.take() {
            // Continuation of an open declaration.
            let (close_at, saw_open) = scan_for_close(line, &mut state, line_start, decl.saw_brace);
            match close_at {
                Some(end_byte) => emit_decl(entries, rel_path, source, lines, &decl, end_byte),
                None => pending = Some(PendingDecl {
                    saw_brace: decl.saw_brace || saw_open,
                    ..decl
                }),
            }
        } else if state.depth == 0 && !state.in_block_comment {
            // Top level. Detect a declaration start first so that a same-line
            // closing brace is attributed to the new declaration; otherwise this
            // call is just plain state maintenance.
            let matched = match_decl_start(line, keywords);
            let (close_at, saw_open) = scan_for_close(line, &mut state, line_start, false);
            if let Some((kind, name)) = matched {
                let decl = PendingDecl {
                    start_byte: line_start + (line.len() - line.trim_start().len()),
                    start_line: idx + 1,
                    kind,
                    name,
                    saw_brace: true, // a same-line close implies a brace was seen.
                };
                match close_at {
                    Some(end_byte) => emit_decl(entries, rel_path, source, lines, &decl, end_byte),
                    None => pending = Some(PendingDecl {
                        saw_brace: saw_open,
                        ..decl
                    }),
                }
            }
        } else {
            // Inside another declaration or a block comment: state maintenance only.
            scan_brace_line(line, &mut state, line_start, |_, _| {});
        }
    }

    // Unterminated declaration at EOF: extend the range to end of source.
    if let Some(decl) = pending {
        emit_decl(entries, rel_path, source, lines, &decl, source.len());
    }
}

/// Build and push a [`SymbolEntry`] for a closed brace-language declaration. The
/// signature is the first physical line cut at its first `{` (or whole header when
/// no brace appears), normalized via [`normalize_signature`].
fn emit_decl(
    entries: &mut Vec<SymbolEntry>,
    rel_path: &str,
    source: &str,
    lines: &[(usize, usize)],
    decl: &PendingDecl,
    end_byte: usize,
) {
    let (start_of_header, header_end) = lines[decl.start_line - 1];
    let header = &source[start_of_header..header_end];
    let cut = header.find('{').unwrap_or(header.len());
    let signature = normalize_signature(&header[..cut]);
    let end_byte = end_byte.min(source.len());
    entries.push(SymbolEntry {
        uri: SymbolUri(format!("repo://{rel_path}#{}", decl.name)),
        rel_path: rel_path.to_string(),
        start_byte: decl.start_byte,
        end_byte,
        start_line: decl.start_line,
        end_line: line_number_of(lines, (end_byte - 1).min(source.len().saturating_sub(1))),
        kind: decl.kind,
        signature,
    });
}

/// Top-level declaration extraction for Python: zero-indent `def`/`class` lines.
/// A block extends over all following blank or indented lines; nested definitions
/// are part of the enclosing block's range. The byte slice ends just past the last
/// non-whitespace character of the final body line.
fn extract_python(
    rel_path: &str,
    source: &str,
    lines: &[(usize, usize)],
    entries: &mut Vec<SymbolEntry>,
) {
    for (idx, &(line_start, _)) in lines.iter().enumerate() {
        let line = &source[line_start..lines[idx].1];
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if indent > 0 {
            continue; // nested scope.
        }
        let Some((kind, name)) = match_python_decl(line) else {
            continue;
        };

        // Extend the block over blank or indented continuation lines.
        let mut last_body = idx;
        let mut j = idx + 1;
        while j < lines.len() {
            let body_line = &source[lines[j].0..lines[j].1];
            if body_line.trim().is_empty() {
                j += 1;
                continue;
            }
            let body_indent = body_line.len() - body_line.trim_start_matches([' ', '\t']).len();
            if body_indent == 0 {
                break; // next top-level statement.
            }
            last_body = j;
            j += 1;
        }

        let (body_start, _body_end) = lines[last_body];
        let body_content = &source[body_start..lines[last_body].1];
        // Just past the last non-whitespace character of the final body line.
        let end_byte = body_start + body_content.trim_end().len();

        let mut signature = line.trim().to_string();
        if signature.ends_with(':') {
            signature.pop();
        }
        let signature = normalize_signature(&signature);

        entries.push(SymbolEntry {
            uri: SymbolUri(format!("repo://{rel_path}#{name}")),
            rel_path: rel_path.to_string(),
            start_byte: line_start, // zero-indent declarations start at column 0.
            end_byte,
            start_line: idx + 1,
            end_line: last_body + 1,
            kind,
            signature,
        });
    }
}

/// Match a Python top-level declaration header after optional modifiers.
fn match_python_decl(line: &str) -> Option<(SymbolKind, String)> {
    let mut rest = line.trim_start();
    while let Some(stripped) = DECL_MODIFIERS.iter().find_map(|m| rest.strip_prefix(*m)) {
        rest = stripped.trim_start();
    }
    if let Some(after) = rest.strip_prefix("def ") {
        return identifier_after(after).map(|name| (SymbolKind::Function, name));
    }
    if let Some(after) = rest.strip_prefix("class ") {
        return identifier_after(after).map(|name| (SymbolKind::Class, name));
    }
    None
}

impl SymbolIndex {
    /// Build an index from in-memory sources `(rel_path, content)` in the given
    /// order; deterministic for a fixed input sequence.
    pub fn build_from_sources(sources: &[(&str, &str)]) -> Self {
        let mut index = Self::default();
        for (rel_path, source) in sources {
            for extracted in extract_symbols_from_source(rel_path, source) {
                index.entries.entry(extracted.uri.clone()).or_insert(extracted);
            }
        }
        index
    }

    /// Build a deterministic [`SymbolIndex`] over `workspace_root`.
    ///
    /// Walks the tree in sorted directory-entry order (so first-wins URI collision
    /// handling is reproducible), skipping hidden entries and [`EXCLUDED_DIRS`],
    /// following no symlinks, and reading only regular files with extensions from
    /// [`SOURCE_EXTENSIONS`]. Unreadable files or subdirectories are skipped; a
    /// failure to read the root itself propagates.
    pub fn build(workspace_root: &Path) -> std::io::Result<Self> {
        let mut index = Self::default();
        collect_entries(workspace_root, workspace_root, &mut index)?;
        Ok(index)
    }
}

fn collect_entries(root: &Path, dir: &Path, index: &mut SymbolIndex) -> std::io::Result<()> {
    let mut dirents: Vec<DirEntry> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
    dirents.sort_by_key(|entry| entry.file_name());

    for entry in dirents {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let name = match entry.file_name().to_str() {
            Some(n) if !n.starts_with('.') => n.to_string(),
            _ => continue,
        };

        if file_type.is_dir() && !file_type.is_symlink() {
            if EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            // Unreadable subdirectories are skipped to keep indexing best-effort.
            let _ = collect_entries(root, &entry.path(), index);
        } else if file_type.is_file() && !file_type.is_symlink() {
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !SOURCE_EXTENSIONS.contains(&extension) {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue; // binary or unreadable file: not indexable.
            };
            let Some(rel_path) = path.strip_prefix(root).ok() else {
                continue;
            };
            let rel_path = rel_path.to_string_lossy().replace('\\', "/");
            for extracted in extract_symbols_from_source(&rel_path, &content) {
                index.entries.entry(extracted.uri.clone()).or_insert(extracted);
            }
        }
    }
    Ok(())
}

/// Fixed memory-protocol header of the Zone 1 block. These four instruction lines
/// are rendered verbatim on every render so the prefix stays byte-stable across turns.
const ZONE1_PROTOCOL_HEADER: &str = "\
<cw_memory_protocol>
- Use <workspace_symbol_map> URIs (repo://<path>#<symbol>) to locate code without bash find/grep/cat.
- Call cw_mount(uri, mode) to load signatures or full implementations into Zone 2 (<mounted_context>).
- Call cw_unmount(uri) when finished with a symbol to keep context lean.
- Call cw_pin(key, rule) to lock discovered architectural invariants.
</cw_memory_protocol>
<workspace_symbol_map>
";

const ZONE1_MAP_CLOSE: &str = "</workspace_symbol_map>";

impl SymbolIndex {
    /// Render the deterministic Zone 1 prefix block (memory protocol + workspace
    /// symbol map), enforcing a `max_tokens` budget via
    /// [`crate::tokenizer::estimate_tokens`].
    ///
    /// Format: the fixed `<cw_memory_protocol>` header, then one line per file in
    /// `BTreeMap` (URI-sorted) order — files without symbols are omitted — of the
    /// form `repo://<rel_path>: <signature> (L<a>-<b>)[, ...]`, closing with
    /// `<\/workspace_symbol_map>` and no trailing newline. File lines are dropped
    /// from the end once the budget would be exceeded; when truncated a
    /// `# ... truncated (<remaining> files)` marker is appended before the closing
    /// tag whenever it still fits in the budget (pathologically small budgets may
    /// omit even the marker, but never emit more file lines than fit).
    pub fn render_zone1_prefix(&self, max_tokens: usize) -> String {
        // Group non-file entries by rel_path; BTreeMap iteration already yields
        // files in deterministic sorted order with symbols sorted inside each.
        let mut groups: Vec<(String, Vec<&SymbolEntry>)> = Vec::new();
        for entry in self.entries.values() {
            if entry.kind == SymbolKind::File {
                continue;
            }
            match groups.last_mut() {
                Some((path, symbols)) if path == &entry.rel_path => symbols.push(entry),
                _ => groups.push((entry.rel_path.clone(), vec![entry])),
            }
        }

        let mut out = ZONE1_PROTOCOL_HEADER.to_string();
        let mut rendered_files = 0usize;
        for (path, symbols) in &groups {
            let line = render_file_line(path, symbols);
            let candidate = format!("{out}{line}\n{ZONE1_MAP_CLOSE}");
            if crate::tokenizer::estimate_tokens(&candidate) as usize > max_tokens {
                break;
            }
            out.push_str(&line);
            out.push('\n');
            rendered_files += 1;
        }

        let remaining = groups.len() - rendered_files;
        if remaining > 0 {
            // Prefer the spec marker when it fits the budget.
            let with_marker = format!(
                "{out}# ... truncated ({remaining} files)\n{ZONE1_MAP_CLOSE}"
            );
            out = if crate::tokenizer::estimate_tokens(&with_marker) as usize <= max_tokens {
                with_marker
            } else {
                format!("{out}{ZONE1_MAP_CLOSE}")
            };
        } else {
            out.push_str(ZONE1_MAP_CLOSE);
        }
        out
    }
}

/// One symbol-map line: `repo://<rel_path>: <sig> (L<a>-<b>)[, ...]`.
fn render_file_line(rel_path: &str, symbols: &[&SymbolEntry]) -> String {
    let mut line = format!("repo://{rel_path}: ");
    for (i, symbol) in symbols.iter().enumerate() {
        if i > 0 {
            line.push_str(", ");
        }
        line.push_str(&symbol.signature);
        line.push_str(&format!(" (L{}-{})", symbol.start_line, symbol.end_line));
    }
    line
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
            ("repo://crates/cw-core/src/lib.rs#control", SymbolKind::Module),
            ("repo://crates/cw-core/src/lib.rs#ir", SymbolKind::Module),
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

    #[test]
    fn rust_source_byte_slices_roundtrip() {
        let src = [
            "//! crate docs",
            "pub mod ir;",
            "",
            "/// A widget.",
            "pub struct Widget {",
            "    pub x: u32,",
            "}",
            "",
            "impl Iterator for Widget {",
            "    fn new() -> Self {",
            "        Self { x: 0 }",
            "    }",
            "}",
            "",
            "fn helper(s: &str) {",
            "    let _ = format!(\"{}{}\", s, \"}\");",
            "    // brace in comment: }",
            "}",
        ]
        .join("\n")
            + "\n";

        let entries = extract_symbols_from_source("crates/demo/src/lib.rs", &src);
        assert_eq!(entries.len(), 5, "{entries:?}");

        // Whole-file entry spans the exact source.
        assert_eq!(entries[0].kind, SymbolKind::File);
        assert_eq!(&src[entries[0].start_byte..entries[0].end_byte], src);
        assert_eq!(entries[0].uri, SymbolUri("repo://crates/demo/src/lib.rs".into()));
        assert_eq!(entries[0].signature, "crates/demo/src/lib.rs");
        assert_eq!(entries[0].end_line, 18);

        let kinds: Vec<SymbolKind> = entries.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                SymbolKind::File,
                SymbolKind::Module,
                SymbolKind::Struct,
                SymbolKind::Impl,
                SymbolKind::Function
            ]
        );

        let mod_ir = &entries[1];
        assert_eq!(&src[mod_ir.start_byte..mod_ir.end_byte], "pub mod ir;");
        assert_eq!(mod_ir.uri, SymbolUri("repo://crates/demo/src/lib.rs#ir".into()));
        assert_eq!(mod_ir.signature, "mod ir");
        assert_eq!((mod_ir.start_line, mod_ir.end_line), (2, 2));

        let widget = &entries[2];
        assert_eq!(
            &src[widget.start_byte..widget.end_byte],
            "pub struct Widget {\n    pub x: u32,\n}"
        );
        assert_eq!(widget.uri, SymbolUri("repo://crates/demo/src/lib.rs#Widget".into()));
        assert_eq!(widget.signature, "struct Widget");
        assert_eq!((widget.start_line, widget.end_line), (5, 7));

        let impl_w = &entries[3];
        assert_eq!(
            &src[impl_w.start_byte..impl_w.end_byte],
            "impl Iterator for Widget {\n    fn new() -> Self {\n        Self { x: 0 }\n    }\n}"
        );
        assert_eq!(impl_w.uri, SymbolUri("repo://crates/demo/src/lib.rs#Iterator".into()));
        assert_eq!(impl_w.signature, "impl Iterator for Widget");
        assert_eq!((impl_w.start_line, impl_w.end_line), (9, 13));

        let helper = &entries[4];
        assert_eq!(
            &src[helper.start_byte..helper.end_byte],
            "fn helper(s: &str) {\n    let _ = format!(\"{}{}\", s, \"}\");\n    // brace in comment: }\n}"
        );
        assert_eq!(helper.signature, "fn helper(s: &str)");
        assert_eq!((helper.start_line, helper.end_line), (15, 18));

        for e in &entries {
            assert!(e.start_byte <= e.end_byte && e.end_byte <= src.len());
        }
    }

    #[test]
    fn kotlin_source_byte_slices_roundtrip() {
        let src = [
            "package demo",
            "",
            "import kotlin.math.max",
            "",
            "/* multi-line comment with { brace",
            "   still open */",
            "data class Widget(val x: Int) {",
            "    fun describe(): String = max(x, 0).toString()",
            "}",
            "",
            "fun main(args: Array<String>) {",
            "    println(Widget(1))",
            "}",
            "",
            "fun top(): Int = 1",
        ]
        .join("\n");

        let entries = extract_symbols_from_source("app/src/main/kt/demo/Widget.kt", &src);
        assert_eq!(entries.len(), 4, "{entries:?}");

        let widget = &entries[1];
        assert_eq!(widget.kind, SymbolKind::Class);
        assert_eq!(widget.uri, SymbolUri("repo://app/src/main/kt/demo/Widget.kt#Widget".into()));
        assert_eq!(&src[widget.start_byte..widget.end_byte], "data class Widget(val x: Int) {\n    fun describe(): String = max(x, 0).toString()\n}");
        assert_eq!(widget.signature, "class Widget(val x: Int)");
        assert_eq!((widget.start_line, widget.end_line), (7, 9));

        let main = &entries[2];
        assert_eq!(main.kind, SymbolKind::Function);
        assert_eq!(&src[main.start_byte..main.end_byte], "fun main(args: Array<String>) {\n    println(Widget(1))\n}");
        assert_eq!(main.signature, "fun main(args: Array<String>)");

        // Brace-less expression-body fun: unterminated at EOF extends to end of source.
        let top = &entries[3];
        assert_eq!(&src[top.start_byte..top.end_byte], "fun top(): Int = 1");
        assert_eq!(top.signature, "fun top(): Int = 1");
        assert_eq!((top.start_line, top.end_line), (15, 15));

        // The nested `describe` fun is not a separate entry.
        let names: Vec<&str> = entries.iter().map(|e| e.uri.0.rsplit('#').next().unwrap()).collect();
        assert_eq!(names, ["repo://app/src/main/kt/demo/Widget.kt", "Widget", "main", "top"]);
    }

    #[test]
    fn python_source_byte_slices_roundtrip() {
        let src = [
            "\"\"\"Module docstring.\"\"\"",
            "",
            "",
            "async def load_config(path: str):",
            "    \"\"\"Load config.\"\"\"",
            "    return {\"path\": path}",
            "",
            "",
            "class Registry:",
            "    \"\"\"Holds items.\"\"\"",
            "",
            "    def add(self, item) -> None:",
            "        self._items.append(item)",
            "",
            "",
            "def main():",
            "    print(\"done\")",
        ]
        .join("\n");

        let entries = extract_symbols_from_source("tools/loader.py", &src);
        assert_eq!(entries.len(), 4, "{entries:?}");

        let load_config = &entries[1];
        assert_eq!(load_config.kind, SymbolKind::Function);
        assert_eq!(&src[load_config.start_byte..load_config.end_byte], "async def load_config(path: str):\n    \"\"\"Load config.\"\"\"\n    return {\"path\": path}");
        assert_eq!(load_config.signature, "def load_config(path: str)");
        assert_eq!((load_config.start_line, load_config.end_line), (4, 6));

        let registry = &entries[2];
        assert_eq!(registry.kind, SymbolKind::Class);
        assert_eq!(registry.uri, SymbolUri("repo://tools/loader.py#Registry".into()));
        // The nested method lives inside the class byte range.
        let slice = &src[registry.start_byte..registry.end_byte];
        assert!(slice.contains("def add(self, item)"));
        assert!(!slice.ends_with('\n'));
        assert_eq!(slice, "class Registry:\n    \"\"\"Holds items.\"\"\"\n\n    def add(self, item) -> None:\n        self._items.append(item)");
        assert_eq!((registry.start_line, registry.end_line), (9, 13));

        let main = &entries[3];
        assert_eq!(&src[main.start_byte..main.end_byte], "def main():\n    print(\"done\")");
        assert_eq!(main.signature, "def main()");

        // The nested `add` method is not a separate entry.
        let names: Vec<&str> = entries.iter().map(|e| e.uri.0.rsplit('#').next().unwrap()).collect();
        assert_eq!(names, ["repo://tools/loader.py", "load_config", "Registry", "main"]);
    }

    #[test]
    fn typescript_javascript_extract_top_level() {
        let src = [
            "export interface Point {",
            "    x: number;",
            "}",
            "",
            "export default class Engine {",
            "    run() { return 1; }",
            "}",
            "",
            "function tick(step = 1) {",
            "    const f = () => 2;",
            "    void f;",
            "}",
        ]
        .join("\n");

        let ts_entries = extract_symbols_from_source("web/src/engine.ts", &src);
        assert_eq!(ts_entries.len(), 4, "{ts_entries:?}");
        let point = &ts_entries[1];
        assert_eq!(point.kind, SymbolKind::Trait);
        assert_eq!(&src[point.start_byte..point.end_byte], "export interface Point {\n    x: number;\n}");
        assert_eq!(point.signature, "interface Point");

        let engine = &ts_entries[2];
        assert_eq!(engine.kind, SymbolKind::Class);
        assert_eq!(&src[engine.start_byte..engine.end_byte], "export default class Engine {\n    run() { return 1; }\n}");
        assert_eq!(engine.signature, "class Engine");

        let tick = &ts_entries[3];
        assert_eq!(&src[tick.start_byte..tick.end_byte], "function tick(step = 1) {\n    const f = () => 2;\n    void f;\n}");
        assert_eq!(tick.signature, "function tick(step = 1)");

        // Same source indexed as plain JS: identical structure (keywords shared).
        let js_entries = extract_symbols_from_source("web/src/engine.js", &src);
        assert_eq!(js_entries.len(), ts_entries.len());
    }

    #[test]
    fn build_walks_sorted_and_is_deterministic() {
        let root = std::env::temp_dir().join(format!("cw_symbol_index_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("src/b.rs"), "fn b_one() {}\n").unwrap();
        fs::write(root.join("src/a.ts"), "export class A {}\n").unwrap();
        fs::write(root.join("target/ignored.rs"), "fn nope() {}\n").unwrap();
        fs::write(root.join(".git/nested.py"), "def nope():\n    pass\n").unwrap();
        fs::write(root.join("node_modules/pkg/x.js"), "function nope() {}\n").unwrap();
        fs::write(root.join("README.md"), "# not indexed (no source extension)\n").unwrap();

        let index = SymbolIndex::build(&root).unwrap();
        let keys: Vec<String> = index.entries.keys().map(|k| k.0.clone()).collect();
        assert_eq!(
            keys,
            vec![
                "repo://src/a.ts",
                "repo://src/a.ts#A",
                "repo://src/b.rs",
                "repo://src/b.rs#b_one",
            ]
        );

        // Independent builds over the same tree are byte-for-byte identical.
        let again = SymbolIndex::build(&root).unwrap();
        assert_eq!(index.entries, again.entries);

        // Extracted slices round-trip against the on-disk file content.
        let a = index.entries.get(&SymbolUri("repo://src/a.ts#A".into())).unwrap();
        assert_eq!(&"export class A {}\n"[a.start_byte..a.end_byte], "export class A {}");
        let b = index.entries.get(&SymbolUri("repo://src/b.rs#b_one".into())).unwrap();
        assert_eq!(&"fn b_one() {}\n"[b.start_byte..b.end_byte], "fn b_one() {}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zone1_prefix_render_matches_spec_format() {
        // Mirrors the spec example: a lib.rs whose only symbol is `mod symbol_index`.
        let index = SymbolIndex::build_from_sources(&[("crates/cw-core/src/lib.rs", "pub mod symbol_index;\n")]);
        let rendered = index.render_zone1_prefix(usize::MAX / 2);
        assert_eq!(
            rendered,
            "<cw_memory_protocol>\n\
             - Use <workspace_symbol_map> URIs (repo://<path>#<symbol>) to locate code without bash find/grep/cat.\n\
             - Call cw_mount(uri, mode) to load signatures or full implementations into Zone 2 (<mounted_context>).\n\
             - Call cw_unmount(uri) when finished with a symbol to keep context lean.\n\
             - Call cw_pin(key, rule) to lock discovered architectural invariants.\n\
             </cw_memory_protocol>\n\
             <workspace_symbol_map>\n\
             repo://crates/cw-core/src/lib.rs: mod symbol_index (L1-1)\n\
             </workspace_symbol_map>"
        );
    }

    #[test]
    fn zone1_prefix_deterministic_across_builds() {
        let root = std::env::temp_dir().join(format!("cw_zone1_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/alpha.rs"),
            "fn a_one() {}\n\nstruct Alpha {\n    x: u8,\n}\n",
        )
        .unwrap();
        fs::write(root.join("src/beta.py"), "def beta():\n    pass\n").unwrap();
        fs::write(root.join("top.js"), "function gamma() {}\nclass Delta {}\n").unwrap();

        let first = SymbolIndex::build(&root).unwrap();
        let second = SymbolIndex::build(&root).unwrap();
        let budget = usize::MAX / 2;
        assert_eq!(first.render_zone1_prefix(budget), second.render_zone1_prefix(budget));

        let rendered = first.render_zone1_prefix(budget);
        assert!(rendered.starts_with(ZONE1_PROTOCOL_HEADER));
        // Files appear in BTreeMap (URI) sorted order; within a file, symbols are
        // URI-sorted too (case-sensitive: `Alpha` precedes `a_one`).
        let alpha_pos = rendered
            .find("repo://src/alpha.rs: struct Alpha (L3-5), fn a_one() (L1-1)")
            .expect("alpha line");
        let beta_pos = rendered
            .find("repo://src/beta.py: def beta() (L1-2)")
            .expect("beta line");
        let top_pos = rendered
            .find("repo://top.js: class Delta (L2-2), function gamma() (L1-1)")
            .expect("top line");
        assert!(alpha_pos < beta_pos && beta_pos < top_pos);
        assert!(!rendered.contains("truncated"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zone1_prefix_respects_token_budget() {
        let root = std::env::temp_dir().join(format!("cw_zone1_budget_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        for name in ["alpha.rs", "beta.rs", "gamma.rs"] {
            let stem = name.strip_suffix(".rs").unwrap_or(name);
            fs::write(
                root.join(format!("src/{name}")),
                format!("fn one_{stem}() {{}}\nfn two_{stem}()\n    {{}}\n"),
            )
            .unwrap();
        }

        let index = SymbolIndex::build(&root).unwrap();
        let full = index.render_zone1_prefix(usize::MAX / 2);
        assert!(!full.contains("truncated"));

        // One token below the full render must drop at least one file line and
        // still respect the budget strictly (monotone estimator guarantees this).
        let tight_budget = crate::tokenizer::estimate_tokens(&full) as usize - 1;
        let truncated = index.render_zone1_prefix(tight_budget);
        assert_ne!(truncated, full);
        assert!(crate::tokenizer::estimate_tokens(&truncated) as usize <= tight_budget);
        // Fewer file lines than the full render.
        let count_map_lines = |s: &str| -> usize {
            s.lines()
                .filter(|l| l.starts_with("repo://"))
                .count()
        };
        assert!(count_map_lines(&truncated) < count_map_lines(&full));

        // The marker, when present, reports the remaining file count.
        if let Some(marker) = truncated.lines().find(|l| l.contains("truncated")) {
            assert!(marker.starts_with("# ... truncated ("), "{marker}");
            assert!(marker.ends_with("files)"), "{marker}");
        }

        // Degenerate budget: the protocol frame is still intact.
        let tiny = index.render_zone1_prefix(4);
        assert!(tiny.starts_with("<cw_memory_protocol>"));
        assert!(tiny.ends_with("</workspace_symbol_map>"));

        let _ = fs::remove_dir_all(&root);
    }
}
