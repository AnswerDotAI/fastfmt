//! fastfmt: rustfmt, then re-join the short constructs the house style keeps on
//! one line. rustfmt (stable) always breaks fn bodies, statement if/else, loop
//! bodies, and short match/struct/enum/impl bodies onto multiple lines; the
//! compaction pass joins any such block back when it is comment-free, within the
//! width cap, and small (fn/impl: 1 item, control-flow blocks: 2 statements,
//! comma-separated bodies: 3 items). Joins run innermost-first to a fixpoint, so
//! nested one-liners collapse fully.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tree_sitter::{Node, Parser};

const DEFAULT_WIDTH: usize = 150; // when neither --width nor a rustfmt.toml max_width applies

fn parser() -> Parser {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    p
}

/// Statement/item count limit for a joinable body, or None when `kind` never joins.
fn join_limit(node: &Node) -> Option<usize> {
    match node.kind() {
        "block" if node.parent().is_some_and(|p| p.kind() == "function_item") => Some(1),
        "block" if node.parent().is_some_and(|p| p.kind() == "match_arm") => None, // arms stay expanded
        "block" => Some(2),
        "match_block" | "field_declaration_list" | "enum_variant_list" => Some(3),
        "declaration_list" => Some(1),
        _ => None,
    }
}

fn has_multiline_leaf(node: Node, src: &str) -> bool {
    let mut c = node.walk();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 && src[n.byte_range()].contains('\n') { return true }
        if n.kind().contains("comment") { return true }
        stack.extend(n.children(&mut c));
    }
    false
}

/// True when `block` ends with a return/break/continue statement, whose
/// rustfmt-added trailing semicolon can drop in the one-line form.
fn ends_jump(node: &Node) -> bool {
    if node.kind() != "block" || node.named_child_count() == 0 { return false }
    let last = node.named_child(node.named_child_count() - 1).unwrap();
    last.kind() == "expression_statement"
        && last.child(0).is_some_and(|c| matches!(c.kind(), "return_expression" | "break_expression" | "continue_expression"))
}

/// `node`'s text joined onto one line, or None when it must stay multi-line:
/// not a joinable kind, too many items, comment-bearing, or over its width cap.
fn joined(node: Node, src: &str, width: usize) -> Option<String> {
    let limit = join_limit(&node)?;
    let text = &src[node.byte_range()];
    if !text.contains('\n') { return None }
    if node.named_child_count() > limit { return None }
    if has_multiline_leaf(node, src) { return None }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        let line = if i > 0 { line.trim_start() } else { line };
        // No space before a continuation that rustfmt broke at an operator or delimiter
        if i > 0 && !line.starts_with(['.', ',', ';', '?', ')', ']']) { out.push(' ') }
        out.push_str(line);
    }
    let out = out.replace(", }", " }");
    let out = if ends_jump(&node) && out.ends_with("; }") { format!("{} }}", &out[..out.len() - 3]) } else { out };
    let prefix = src[..node.start_byte()].rfind('\n').map_or(0, |p| node.start_byte() - p - 1);
    let suffix = src[node.end_byte()..].find('\n').unwrap_or(0);
    if prefix + out.len() + suffix > width { return None }
    Some(out)
}

/// One compaction round: join every innermost joinable block, return the new text.
fn compact_round(src: &str, width: usize) -> String {
    let tree = parser().parse(src, None).unwrap();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = vec![];
    let mut stack = vec![tree.root_node()];
    let mut c = tree.root_node().walk();
    while let Some(n) = stack.pop() {
        if let Some(j) = joined(n, src, width) {
            edits.push((n.byte_range(), j));
            continue; // children are inside the joined text; skip them
        }
        stack.extend(n.children(&mut c));
    }
    edits.sort_by(|a, b| b.0.start.cmp(&a.0.start));
    let mut out = src.to_string();
    for (r, j) in edits { out.replace_range(r, &j) }
    out
}

pub fn compact(src: &str, width: usize) -> String {
    let mut cur = src.to_string();
    for _ in 0..5 {
        let next = compact_round(&cur, width);
        if next == cur { break }
        cur = next;
    }
    cur
}

/// Format `src` with rustfmt (stdin mode, sharing our width cap), then compact.
fn fastfmt(src: &str, edition: &str, width: usize) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition, "--emit", "stdout", "--config", &format!("max_width={width},use_small_heuristics=Max")])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("failed to run rustfmt: {e}"))?;
    child.stdin.take().unwrap().write_all(src.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
    Ok(compact(&String::from_utf8_lossy(&out.stdout), width))
}

/// The value at `key` in the nearest `file` at or above `path`, e.g. the edition
/// from Cargo.toml or max_width from rustfmt.toml.
fn toml_lookup(path: &Path, file: &str, key: &str) -> Option<String> {
    for dir in path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).ancestors() {
        if let Ok(t) = std::fs::read_to_string(dir.join(file)) {
            return t.lines().find(|l| l.trim_start().starts_with(key))
                .and_then(|l| l.split('=').nth(1))
                .map(|v| v.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn edition_for(path: &Path) -> String { toml_lookup(path, "Cargo.toml", "edition").unwrap_or_else(|| "2021".into()) }

/// `--width` beats the nearest rustfmt.toml's max_width, which beats the default.
fn width_for(path: &Path, flag: Option<usize>) -> usize {
    flag.or_else(|| toml_lookup(path, "rustfmt.toml", "max_width").and_then(|w| w.parse().ok())).unwrap_or(DEFAULT_WIDTH)
}

fn rs_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "rs") { out.push(path.to_path_buf()) }
        return;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "target" || name.starts_with('.') && name.len() > 1 { return }
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() { rs_files(&e.path(), out) }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "fastfmt") { args.remove(0); } // invoked as `cargo fastfmt`
    let check = args.iter().any(|a| a == "--check");
    let width_flag = args.iter().position(|a| a == "--width")
        .and_then(|i| args.get(i + 1)).and_then(|w| w.parse().ok());
    let mut paths: Vec<PathBuf> = vec![];
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue }
        match a.as_str() {
            "--check" => {}
            "--width" => skip = true,
            _ if i > 0 && args[i - 1] == "--width" => {}
            p => paths.push(p.into()),
        }
    }
    if paths.is_empty() { paths.push(".".into()) }
    let mut files = vec![];
    for p in &paths { rs_files(p, &mut files) }
    files.sort();
    let mut dirty = vec![];
    for f in &files {
        let src = match std::fs::read_to_string(f) { Ok(s) => s, Err(e) => { eprintln!("{}: {e}", f.display()); std::process::exit(2) } };
        match fastfmt(&src, &edition_for(f), width_for(f, width_flag)) {
            Ok(new) if new != src => {
                if check { dirty.push(f) } else if let Err(e) = std::fs::write(f, &new) { eprintln!("{}: {e}", f.display()); std::process::exit(2) }
            }
            Ok(_) => {}
            Err(e) => { eprintln!("{}: {e}", f.display()); std::process::exit(2) }
        }
    }
    if check && !dirty.is_empty() {
        for f in dirty { println!("would reformat {}", f.display()) }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::compact;

    #[test]
    fn joins_house_shapes() {
        let cases = [
            ("fn f(x: u8) -> u8 {\n    if x > 1 {\n        return 1;\n    }\n    0\n}\n",
             "fn f(x: u8) -> u8 {\n    if x > 1 { return 1 }\n    0\n}\n"),
            ("fn time(&self) -> f64 {\n    self.core.time()\n}\n",
             "fn time(&self) -> f64 { self.core.time() }\n"),
            ("fn f(c: bool, e: &mut E, h: H) {\n    if c {\n        e.writer = Some(h)\n    } else {\n        e.reader = Some(h)\n    }\n}\n",
             "fn f(c: bool, e: &mut E, h: H) { if c { e.writer = Some(h) } else { e.reader = Some(h) } }\n"),
            ("struct FdEntry<H> {\n    reader: Option<H>,\n    writer: Option<H>,\n}\n",
             "struct FdEntry<H> { reader: Option<H>, writer: Option<H> }\n"),
            ("enum Rt {\n    Owned(Runtime),\n    Borrowed(Handle),\n}\n",
             "enum Rt { Owned(Runtime), Borrowed(Handle) }\n"),
            ("fn drain(&self, q: &mut Q) {\n    while let Some(h) = q.pop_front() {\n        ready.push_back(h)\n    }\n}\n",
             "fn drain(&self, q: &mut Q) { while let Some(h) = q.pop_front() { ready.push_back(h) } }\n"),
            ("impl<H> Default for FdEntry<H> {\n    fn default() -> Self {\n        Self { reader: None, writer: None }\n    }\n}\n",
             "impl<H> Default for FdEntry<H> { fn default() -> Self { Self { reader: None, writer: None } } }\n"),
            ("fn h(&self) -> Handle {\n    match self {\n        Rt::Owned(r) => r.handle().clone(),\n        Rt::Borrowed(h) => h.clone(),\n    }\n}\n",
             "fn h(&self) -> Handle { match self { Rt::Owned(r) => r.handle().clone(), Rt::Borrowed(h) => h.clone() } }\n"),
        ];
        for (src, want) in cases { assert_eq!(compact(src, 150), want, "src: {src}") }
    }

    #[test]
    fn keeps_what_must_stay() {
        for src in [
            "fn f() {\n    a();\n    b();\n}\n",                       // two statements in a fn body
            "fn f() {\n    // why\n    a()\n}\n",                      // comment must survive
            "fn f() {\n    let s = \"a\nb\";\n}\n",                    // multiline string literal
        ] { assert_eq!(compact(src, 150), src) }
        let long = format!("fn f() {{\n    {}()\n}}\n", "x".repeat(160));
        assert_eq!(compact(&long, 150), long);                    // width cap
    }
}
