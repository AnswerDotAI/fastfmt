//! fastfmt: rustfmt, then re-join the short constructs the house style keeps on
//! one line. rustfmt (stable) always breaks fn bodies, statement if/else, loop
//! bodies, and short match/struct/enum/impl bodies onto multiple lines; the
//! compaction pass joins any such block back when it is comment-free, within the
//! width cap, and small (code blocks: 1 expression, fn/impl: 1 item,
//! comma-separated bodies: 3 items). Joins run innermost-first to a fixpoint, so
//! nested one-liners collapse fully.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tree_sitter::{Node, Parser};

const DEFAULT_WIDTH: usize = 160; // when neither --width nor a rustfmt.toml max_width applies

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
        "block" => Some(1),
        "match_block" | "field_declaration_list" | "enum_variant_list" => Some(3),
        "declaration_list" => Some(1),
        _ => None,
    }
}

fn has_multiline_leaf(node: Node, src: &str) -> bool {
    let mut c = node.walk();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 && src[n.byte_range()].contains('\n') { return true; }
        if n.kind().contains("comment") { return true; }
        stack.extend(n.children(&mut c));
    }
    false
}

/// True when `node` contains another block which has not yet joined. Keeping the
/// outer node expanded lets inner blocks decide independently whether they may
/// join, then a later compaction round can absorb the result.
fn has_multiline_block(node: Node, src: &str) -> bool {
    let mut c = node.walk();
    let mut stack: Vec<_> = node.children(&mut c).collect();
    while let Some(n) = stack.pop() {
        if n.kind() == "block" && src[n.byte_range()].contains('\n') { return true; }
        stack.extend(n.children(&mut c));
    }
    false
}

/// `node`'s text joined onto one line, or None when it must stay multi-line:
/// not a joinable kind, too many items, comment-bearing, or over its width cap.
fn joined(node: Node, src: &str, width: usize) -> Option<String> {
    let limit = join_limit(&node)?;
    let text = &src[node.byte_range()];
    if !text.contains('\n') { return None; }
    if node.named_child_count() > limit { return None; }
    if has_multiline_leaf(node, src) { return None; }
    if has_multiline_block(node, src) { return None; }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        let line = if i > 0 { line.trim_start() } else { line };
        // No space before a continuation that rustfmt broke at an operator or delimiter
        if i > 0 && !line.starts_with(['.', ',', ';', '?', ')', ']']) { out.push(' ') }
        out.push_str(line);
    }
    let out = out.replace(", }", " }");
    let prefix = src[..node.start_byte()].rfind('\n').map_or(0, |p| node.start_byte() - p - 1);
    let suffix = src[node.end_byte()..].find('\n').unwrap_or(0);
    if prefix + out.len() + suffix > width { return None; }
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

fn statement_if(node: Node, src: &str) -> bool {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if parent.kind() == "expression_statement" { return parent.next_named_sibling().is_some() || src[parent.byte_range()].trim_end().ends_with(';'); }
        if parent.kind() != "if_expression" { return false; }
        cur = parent;
    }
    false
}

/// Put each `else` in a statement-position if/else chain on its own line.
fn split_statement_elses(src: &str) -> String {
    let tree = parser().parse(src, None).unwrap();
    let mut edits: Vec<(std::ops::Range<usize>, String)> = vec![];
    let mut stack = vec![tree.root_node()];
    let mut c = tree.root_node().walk();
    while let Some(n) = stack.pop() {
        if n.kind() == "if_expression" && statement_if(n, src) {
            let indent = " ".repeat(n.start_position().column);
            let mut branch = n;
            loop {
                let Some(consequence) = branch.child_by_field_name("consequence") else { break };
                let Some(alternative) = branch.child_by_field_name("alternative") else { break };
                let gap = consequence.end_byte()..alternative.start_byte();
                if !src[gap.clone()].contains('\n') && src[gap.clone()].trim().is_empty() { edits.push((gap, format!("\n{indent}"))) }
                let Some(next) = alternative.named_child(0).filter(|c| c.kind() == "if_expression") else { break };
                branch = next;
            }
        }
        stack.extend(n.children(&mut c));
    }
    edits.sort_by(|a, b| b.0.start.cmp(&a.0.start));
    let mut out = src.to_string();
    for (r, replacement) in edits { out.replace_range(r, &replacement) }
    out
}

pub fn compact(src: &str, width: usize) -> String {
    let mut cur = src.to_string();
    for _ in 0..5 {
        let next = compact_round(&cur, width);
        if next == cur { break; }
        cur = next;
    }
    split_statement_elses(&cur)
}

/// Format `src` with rustfmt (stdin mode, sharing our width cap), then compact.
fn fastfmt(src: &str, edition: &str, width: usize) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition, "--emit", "stdout", "--config", &format!("max_width={width},use_small_heuristics=Max,disable_all_formatting=false")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run rustfmt: {e}"))?;
    child.stdin.take().unwrap().write_all(src.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into_owned()); }
    Ok(compact(&String::from_utf8_lossy(&out.stdout), width))
}

/// The value at `key` in the nearest `file` at or above `path`, e.g. the edition
/// from Cargo.toml or max_width from rustfmt.toml.
fn toml_lookup(path: &Path, file: &str, key: &str) -> Option<String> {
    for dir in path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).ancestors() {
        if let Ok(t) = std::fs::read_to_string(dir.join(file)) {
            return t.lines().find(|l| l.trim_start().starts_with(key)).and_then(|l| l.split('=').nth(1)).map(|v| v.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn edition_for(path: &Path) -> String { toml_lookup(path, "Cargo.toml", "edition").unwrap_or_else(|| "2021".into()) }

/// `--width` beats the nearest rustfmt.toml's max_width, which beats the default.
fn width_for(path: &Path, flag: Option<usize>) -> usize {
    flag.or_else(|| toml_lookup(path, "rustfmt.toml", "max_width").and_then(|w| w.parse().ok())).unwrap_or(DEFAULT_WIDTH)
}

fn guarded_config(src: &str) -> String {
    let mut offset = 0;
    for part in src.split_inclusive('\n') {
        let line = part.trim_end_matches(['\r', '\n']);
        if let Some(eq) = line.find('=').filter(|&eq| line[..eq].trim() == "disable_all_formatting") {
            let value_end = line[eq + 1..].find('#').map_or(line.len(), |i| eq + 1 + i);
            let replacement = if value_end < line.len() { " true " } else { " true" };
            let mut out = src.to_string();
            out.replace_range(offset + eq + 1..offset + value_end, replacement);
            return out;
        }
        offset += part.len();
    }
    let mut out = src.to_string();
    if !out.is_empty() && !out.ends_with('\n') { out.push('\n') }
    out.push_str("disable_all_formatting = true\n");
    out
}

fn config_path(path: &Path) -> PathBuf {
    let start = if path.is_file() { path.parent().unwrap_or(path) } else { path };
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    start.ancestors().find(|p| p.join("Cargo.toml").is_file()).unwrap_or(&start).join("rustfmt.toml")
}

/// Ensure ordinary rustfmt is disabled for each target project. Returns paths
/// which needed an update; in check mode they are reported but left untouched.
fn guard_rustfmt(paths: &[PathBuf], check: bool) -> Result<Vec<PathBuf>, String> {
    let mut configs: Vec<_> = paths.iter().map(|p| config_path(p)).collect();
    configs.sort();
    configs.dedup();
    let mut dirty = vec![];
    for path in configs {
        let old = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let new = guarded_config(&old);
        if new == old { continue; }
        dirty.push(path.clone());
        if !check { std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))? }
    }
    Ok(dirty)
}

fn rs_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "rs") { out.push(path.to_path_buf()) }
        return;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "target" || name.starts_with('.') && name.len() > 1 { return; }
    if let Ok(rd) = std::fs::read_dir(path) { for e in rd.flatten() { rs_files(&e.path(), out) } }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "fastfmt") { args.remove(0); } // invoked as `cargo fastfmt`
    let check = args.iter().any(|a| a == "--check");
    let width_flag = args.iter().position(|a| a == "--width").and_then(|i| args.get(i + 1)).and_then(|w| w.parse().ok());
    let mut paths: Vec<PathBuf> = vec![];
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        match a.as_str() {
            "--check" => {}
            "--width" => skip = true,
            _ if i > 0 && args[i - 1] == "--width" => {}
            p => paths.push(p.into()),
        }
    }
    if paths.is_empty() { paths.push(".".into()) }
    let dirty_configs = match guard_rustfmt(&paths, check) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2)
        }
    };
    let mut files = vec![];
    for p in &paths { rs_files(p, &mut files) }
    files.sort();
    let mut dirty = vec![];
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: {e}", f.display());
                std::process::exit(2)
            }
        };
        match fastfmt(&src, &edition_for(f), width_for(f, width_flag)) {
            Ok(new) if new != src => {
                if check { dirty.push(f) } else if let Err(e) = std::fs::write(f, &new) {
                    eprintln!("{}: {e}", f.display());
                    std::process::exit(2)
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {e}", f.display());
                std::process::exit(2)
            }
        }
    }
    if check && (!dirty.is_empty() || !dirty_configs.is_empty()) {
        for f in dirty_configs { println!("would update {}", f.display()) }
        for f in dirty { println!("would reformat {}", f.display()) }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{compact, guarded_config};

    #[test]
    fn guards_rustfmt_config() {
        assert_eq!(guarded_config(""), "disable_all_formatting = true\n");
        assert_eq!(guarded_config("max_width = 120\n"), "max_width = 120\ndisable_all_formatting = true\n");
        assert_eq!(guarded_config("disable_all_formatting = false # no\n"), "disable_all_formatting = true # no\n");
        assert_eq!(guarded_config("disable_all_formatting = true\n"), "disable_all_formatting = true\n");
    }

    #[test]
    fn joins_house_shapes() {
        let cases = [
            ("fn f(x: u8) -> u8 {\n    if x > 1 {\n        return 1;\n    }\n    0\n}\n", "fn f(x: u8) -> u8 {\n    if x > 1 { return 1; }\n    0\n}\n"),
            ("fn f(ready: bool) {\n    if ready {\n        notify();\n    }\n}\n", "fn f(ready: bool) { if ready { notify(); } }\n"),
            (
                "fn f(fds: Fds) {\n    for fd in fds {\n        let _ = self.poller.delete(borrowed(fd));\n    }\n}\n",
                "fn f(fds: Fds) { for fd in fds { let _ = self.poller.delete(borrowed(fd)); } }\n",
            ),
            ("fn time(&self) -> f64 {\n    self.core.time()\n}\n", "fn time(&self) -> f64 { self.core.time() }\n"),
            (
                "fn f(c: bool, e: &mut E, h: H) {\n    if c {\n        e.writer = Some(h)\n    } else {\n        e.reader = Some(h)\n    }\n}\n",
                "fn f(c: bool, e: &mut E, h: H) { if c { e.writer = Some(h) } else { e.reader = Some(h) } }\n",
            ),
            (
                "fn f(c: bool) {\n    if c {\n        a()\n    } else {\n        b()\n    }\n    done()\n}\n",
                "fn f(c: bool) {\n    if c { a() }\n    else { b() }\n    done()\n}\n",
            ),
            ("struct FdEntry<H> {\n    reader: Option<H>,\n    writer: Option<H>,\n}\n", "struct FdEntry<H> { reader: Option<H>, writer: Option<H> }\n"),
            ("enum Rt {\n    Owned(Runtime),\n    Borrowed(Handle),\n}\n", "enum Rt { Owned(Runtime), Borrowed(Handle) }\n"),
            (
                "fn drain(&self, q: &mut Q) {\n    while let Some(h) = q.pop_front() {\n        ready.push_back(h)\n    }\n}\n",
                "fn drain(&self, q: &mut Q) { while let Some(h) = q.pop_front() { ready.push_back(h) } }\n",
            ),
            (
                "impl<H> Default for FdEntry<H> {\n    fn default() -> Self {\n        Self { reader: None, writer: None }\n    }\n}\n",
                "impl<H> Default for FdEntry<H> { fn default() -> Self { Self { reader: None, writer: None } } }\n",
            ),
            (
                "fn h(&self) -> Handle {\n    match self {\n        Rt::Owned(r) => r.handle().clone(),\n        Rt::Borrowed(h) => h.clone(),\n    }\n}\n",
                "fn h(&self) -> Handle { match self { Rt::Owned(r) => r.handle().clone(), Rt::Borrowed(h) => h.clone() } }\n",
            ),
        ];
        for (src, want) in cases { assert_eq!(compact(src, 150), want, "src: {src}") }
    }

    #[test]
    fn keeps_what_must_stay() {
        for src in [
            "fn f() {\n    a();\n    b();\n}\n",                                                   // two statements in a fn body
            "fn f(ready: bool) {\n    if ready {\n        notify();\n        return;\n    }\n}\n", // neither an inner nor enclosing block may absorb two statements
            "fn f(r: R) -> u8 {\n    match r {\n        Err(e) => {\n            log(e);\n            return 1;\n        }\n        Ok(v) => v,\n    }\n}\n", // multi-statement arm: neither the match nor an enclosing block joins
            "fn f() {\n    // why\n    a()\n}\n",   // comment must survive
            "fn f() {\n    let s = \"a\nb\";\n}\n", // multiline string literal
        ] { assert_eq!(compact(src, 150), src) }
        let long = format!("fn f() {{\n    {}()\n}}\n", "x".repeat(160));
        assert_eq!(compact(&long, 150), long); // width cap
    }
}
