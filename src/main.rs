//! fastfmt: rustfmt followed by syntax-aware small-body joining and continuation packing. Meaningful multiline bodies and protected text remain boundaries; accepted edits remove existing breaks until no further compaction is possible.
use ignore::WalkBuilder;
use pulldown_cmark::{Event, Options, Tag, TagEnd};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tree_sitter::{Node, Parser, Tree};

const DEFAULT_WIDTH: usize = 160; // when neither --width nor a rustfmt.toml max_width applies
const COMPACT_WIDTH: usize = 120; // budget for each joined body, and for the call/chain enclosing a joined closure body

fn parse(src: &str) -> Tree {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
    p.parse(src, None).unwrap()
}

/// Visit `root` and its descendants, entering a node's children only when `f` returns true.
fn walk<'a>(root: Node<'a>, mut f: impl FnMut(Node<'a>) -> bool) {
    let mut stack = vec![root];
    let mut cursor = root.walk();
    while let Some(n) = stack.pop() { if f(n) { stack.extend(n.children(&mut cursor)); } }
}

/// Replace each of the nonoverlapping `edits` ranges of `src`.
fn apply(src: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|edit| Reverse(edit.0.start));
    let mut out = src.to_string();
    for (range, text) in edits { out.replace_range(range, &text) }
    out
}

fn line_start(src: &str, pos: usize) -> usize { src[..pos].rfind('\n').map_or(0, |p| p + 1) }

/// The position of the newline ending the line containing `pos`, or the end of `src`.
fn line_end(src: &str, pos: usize) -> usize { src[pos..].find('\n').map_or(src.len(), |p| pos + p) }

fn last_line(text: &str) -> &str { &text[line_start(text, text.len())..] }

fn indent_width(line: &str) -> usize { line.len() - line.trim_start().len() }

fn one_row(node: Node) -> bool { node.start_position().row == node.end_position().row }

fn terminated(statement: Node) -> bool { statement.child(statement.child_count() - 1).is_some_and(|n| n.kind() == ";") }

/// Whether a final block child is an expression rather than a terminated statement or item.
fn is_tail_expression(mut node: Node) -> bool {
    if node.kind() == "expression_statement" {
        if terminated(node) { return false; }
        node = node.named_child(0).unwrap();
    }
    let language = node.language();
    ["_expression", "_literal"].iter().any(|kind| language.subtypes_for_supertype(language.id_for_node_kind(kind, true)).contains(&node.kind_id()))
}

/// Whether a final block child is `break`, `continue` or `return` as a terminated statement.
fn is_exit(node: Node) -> bool {
    node.kind() == "expression_statement"
        && node.named_child(0).is_some_and(|n| matches!(n.kind(), "break_expression" | "continue_expression" | "return_expression"))
}

/// Whether `node` is an if/else chain in statement position, whose `else` `format_elses` puts on its own line.
fn else_statement(node: Node) -> bool { node.kind() == "if_expression" && node.child_by_field_name("alternative").is_some() && statement_if(node) }

/// How a candidate compacts: a `Body` joins whole, a `List` packs inside its outer breaks, and a `Run` packs throughout.
#[derive(PartialEq)]
enum Shape { Body, List, Run }

/// How `node` may compact, or None when it never does.
fn shape(node: Node) -> Option<Shape> {
    let body = |limit: usize| (node.named_child_count() <= limit).then_some(Shape::Body);
    match node.kind() {
        "block" if node.named_child(0).and_then(|n| n.named_child(0)).is_some_and(else_statement) => None,
        "block" => body(if node.named_child(1).is_some_and(|n| is_tail_expression(n) || is_exit(n)) { 2 } else { 1 }), "declaration_list" => body(1),
        "token_tree" if simple_macro_body(node) => Some(Shape::Body),
        "token_tree" => (node.child(0).is_some_and(|n| n.kind() != "{") && node.children(&mut node.walk()).any(|n| n.kind() == ",")).then_some(Shape::List),
        "field_declaration_list" | "use_list" | "array_expression" | "enum_variant_list" | "match_block" => Some(Shape::List),
        "function_item" | "function_signature_item" | "impl_item" | "struct_item" | "use_declaration" | "parameters" | "arguments" => Some(Shape::Run),
        _ => is_tail_expression(node).then_some(Shape::Run)
    }
}

/// Opaque macro arms can join only without nested token groups or multiple terminated statements.
fn simple_macro_body(node: Node) -> bool {
    if node.child(0).is_none_or(|n| n.kind() != "{") || node.prev_sibling().is_none_or(|n| n.kind() != "=>") { return false; }
    let mut cursor = node.walk();
    let mut semicolons = 0;
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "token_tree" | "token_repetition") { return false; }
        semicolons += usize::from(child.kind() == ";");
    }
    semicolons <= 1
}

fn multiline_body(node: Node, src: &str) -> bool {
    matches!(
        node.kind(),
        "block" | "match_block" | "field_declaration_list" | "enum_variant_list" | "declaration_list" | "array_expression" | "use_list" | "token_tree"
    ) && src[node.byte_range()].contains('\n')
}

/// One syntax scan supplies both small-body joining and continuation packing.
struct Layout<'a> {
    node: Node<'a>, src: &'a str, prefix: &'a str, suffix: &'a str, tight: bool, islands: Vec<Node<'a>>, protected: Vec<usize>, commas: Vec<usize>,
    openings: Vec<usize>
}

impl<'a> Layout<'a> {
    fn new(node: Node<'a>, src: &'a str) -> Self {
        let (start, end) = (node.start_byte(), node.end_byte());
        let mut layout = Self { node, src, prefix: &src[line_start(src, start)..start], suffix: &src[end..line_end(src, end)],
            tight: matches!(node.kind(), "use_declaration" | "use_list"), islands: vec![], protected: vec![], commas: vec![], openings: vec![] };
        walk(node, |n| {
            if n != node && multiline_body(n, src) && !layout.islands.iter().any(|b| b.byte_range().contains(&n.start_byte())) { layout.islands.push(n); }
            if n.kind().contains("comment") || n.child_count() == 0 && src[n.byte_range()].contains('\n') { layout.protected.push(n.start_byte()); }
            if matches!(n.kind(), "parameters" | "arguments") && src.as_bytes().get(n.start_byte() + 1) == Some(&b'\n') {
                layout.openings.push(n.start_byte() + 1);
            }
            if n.kind() == "," {
                let parent = n.parent();
                if parent.is_some_and(|p| {
                    matches!(
                        p.kind(), "arguments" | "parameters" | "use_list" | "match_arm" | "where_clause" | "field_declaration_list" | "field_initializer_list"
                            | "enum_variant_list"
                    ) || p.kind() == "tuple_expression" && p.named_child(1).is_some_and(is_tail_expression)
                }) && n.next_sibling().or_else(|| parent.filter(|p| matches!(p.kind(), "match_arm" | "where_clause")).and_then(|p| p.next_sibling()))
                    .is_some_and(|n| matches!(n.kind(), ")" | "]" | "}" | "block" | "declaration_list")) { layout.commas.push(n.start_byte()); }
            }
            true
        });
        layout.islands.sort_by_key(Node::start_byte);
        layout.commas.sort_by_key(|&p| Reverse(p));
        layout.openings.sort_unstable();
        layout
    }

    /// Remove only optional syntax commas; opaque macro groups retain every token.
    fn text(&self, range: Range<usize>) -> String {
        let mut text = self.src[range.clone()].to_string();
        for &pos in &self.commas { if range.contains(&pos) { text.remove(pos - range.start); } }
        text
    }

    /// Pack `range` onto `out`, keeping an argument/parameter opening break in it when the rest cannot follow on the current line.
    fn pack(&self, range: Range<usize>, out: &mut String, width: usize, suffix: &str) {
        let text = self.text(range.clone()) + suffix;
        let opening = self.openings.iter().find(|&&p| range.contains(&p)).filter(|_| last_line(out).len() + self.flatten(&text).len() > width)
            .map(|&p| self.src[range.start..p].matches('\n').count() + 1);
        pack_lines(&text, out, width, self.tight, opening);
    }

    fn flatten(&self, text: &str) -> String {
        let mut out = String::new();
        pack_lines(text, &mut out, usize::MAX, self.tight, None);
        out
    }

    fn flat(&self) -> String { self.flatten(&self.text(self.node.byte_range())) }

    fn fits(&self, text: &str, width: usize) -> bool { self.prefix.len() + text.len() + self.suffix.len() <= width }
}

/// Greedily erase existing continuation breaks, never introducing a breakpoint or splitting a token.
fn pack_lines(text: &str, out: &mut String, width: usize, tight_braces: bool, keep_break: Option<usize>) {
    for (i, line) in text.lines().enumerate() {
        if i == 0 { out.push_str(line); continue; }
        let trimmed = line.trim_start();
        let space = !trimmed.starts_with(['.', ',', ';', '?', ')', ']']) && !out.ends_with(['(', '['])
            && !(tight_braces && (out.ends_with('{') || trimmed.starts_with('}'))) || out.ends_with(',') && trimmed.starts_with("..");
        if keep_break != Some(i) && last_line(out).len() + usize::from(space) + trimmed.len() <= width {
            if space { out.push(' '); }
            out.push_str(trimmed);
        } else {
            out.push('\n');
            out.push_str(line);
        }
    }
}

/// The enclosing call/chain, including outer calls and expression wrappers but not the containing statement or block.
fn call_context(mut node: Node) -> Node {
    let mut context = node;
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "block" | "let_declaration" | "expression_statement") { break; }
        if parent.kind() == "call_expression" { context = parent; }
        node = parent;
    }
    context
}

/// Whether a closure body's enclosing call/chain fits the compact width; other nodes always pass.
fn closure_fits(node: Node, src: &str, width: usize) -> bool {
    if node.kind() != "block" || node.parent().is_none_or(|p| p.kind() != "closure_expression") { return true; }
    let context = Layout::new(call_context(node), src);
    context.protected.is_empty() && context.fits(&context.flat(), width.min(COMPACT_WIDTH))
}

/// `node` with existing breaks removed around preserved multiline islands, or None when no break can go.
fn compacted(node: Node, src: &str, width: usize) -> Option<String> {
    if !src[node.byte_range()].contains('\n') { return None; }
    let shape = shape(node)?;
    let layout = Layout::new(node, src);
    if layout.protected.iter().any(|p| !layout.islands.iter().any(|b| b.byte_range().contains(p))) { return None; }
    if layout.islands.is_empty() && (shape == Shape::Body || node.kind() != "token_tree") {
        let flat = layout.flat();
        if shape == Shape::Body {
            return (flat.len() <= width.min(COMPACT_WIDTH) && layout.fits(&flat, width) && closure_fits(node, src, width)).then_some(flat);
        }
        if layout.fits(&flat, width) { return Some(flat); }
    }
    if shape == Shape::Body || matches!(node.kind(), "match_block" | "enum_variant_list") && !layout.islands.is_empty() { return None; }
    let mut out = layout.prefix.to_string();
    let (mut start, mut end) = (node.start_byte(), node.end_byte());
    if shape == Shape::List {
        let first = src[node.byte_range()].find('\n').unwrap() + node.start_byte();
        let last = src[node.byte_range()].rfind('\n').unwrap() + node.start_byte();
        if first == last { return None; }
        out.push_str(&src[start..first + 1]);
        (start, end) = (first + 1, last);
    }
    for block in &layout.islands {
        // An island that crosses a list's first or last line can't stay intact.
        if block.start_byte() < start || block.end_byte() > end { return None; }
        layout.pack(start..block.start_byte() + 1, &mut out, width, "");
        let old = indent_width(&src[line_start(src, block.end_byte() - 1)..block.end_byte() - 1]);
        let new = if block.parent().is_some_and(|p| p.kind().ends_with("_item")) { old } else { indent_width(last_line(&out)) };
        let body = &src[block.start_byte() + 1..block.end_byte() - 1];
        if new == old { out.push_str(body); } else {
            if layout.protected.iter().any(|p| block.byte_range().contains(p)) { return None; }
            out.push_str(&body.replace(&format!("\n{}", " ".repeat(old)), &format!("\n{}", " ".repeat(new))));
        }
        start = block.end_byte() - 1;
    }
    if shape == Shape::List {
        layout.pack(start..end, &mut out, width, "");
        out.push_str(&src[end..node.end_byte()]);
    } else {
        layout.pack(start..end, &mut out, width, layout.suffix);
        out.truncate(out.len() - layout.suffix.len());
    }
    let out = out[layout.prefix.len()..].to_string();
    // Strict newline removal makes the fixpoint monotonic; indentation-only edits are declined.
    (out.matches('\n').count() < src[node.byte_range()].matches('\n').count()).then_some(out)
}

/// Apply nonoverlapping top-down candidates; their children are reconsidered in later rounds.
fn compact_round(src: &str, width: usize) -> String {
    let tree = parse(src);
    let mut edits = vec![];
    walk(tree.root_node(), |n| {
        let Some(text) = compacted(n, src, width) else { return true };
        edits.push((n.byte_range(), text));
        false
    });
    apply(src, edits)
}

fn statement_if(node: Node) -> bool {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if parent.kind() == "expression_statement" { return parent.next_named_sibling().is_some() || terminated(parent); }
        if parent.kind() != "if_expression" { return false; }
        cur = parent;
    }
    false
}

/// Separate compact branch runs, keeping multiline branch braces attached to their else.
fn format_elses(src: &str, width: usize) -> String {
    let tree = parse(src);
    let mut edits = vec![];
    walk(tree.root_node(), |n| {
        if n.kind() != "if_expression" || n.parent().is_some_and(|p| p.kind() == "else_clause") { return true; }
        let statement = statement_if(n);
        if !statement && one_row(n) && n.end_position().column + line_end(src, n.end_byte()) - n.end_byte() <= width { return true; }
        let indent = " ".repeat(if statement { n.start_position().column } else { indent_width(&src[line_start(src, n.start_byte())..n.start_byte()]) });
        let mut branch = n;
        while let Some(consequence) = branch.child_by_field_name("consequence") {
            let Some(alternative) = branch.child_by_field_name("alternative") else { break };
            let Some(next) = alternative.named_child(0) else { break };
            let gap = consequence.end_byte()..alternative.start_byte();
            let separate = one_row(consequence) && !(next.kind() == "block" && !one_row(next));
            if src[gap.clone()].trim().is_empty() {
                let replacement = if separate { format!("\n{indent}") } else { " ".into() };
                if src[gap.clone()] != replacement { edits.push((gap, replacement)); }
            }
            if next.kind() != "if_expression" { break; }
            branch = next;
        }
        true
    });
    apply(src, edits)
}

pub fn compact(src: &str, width: usize) -> String {
    let mut cur = src.to_string();
    loop {
        let next = compact_round(&cur, width);
        if next == cur { break; }
        cur = next;
    }
    format_elses(&cur, width)
}

struct CommentLine<'a> { start: usize, body: usize, end: usize, prefix: &'a str }

/// Source ranges covering soft breaks and the following comment/container prefixes.
fn comment_joins(src: &str, lines: &[CommentLine]) -> Vec<std::ops::Range<usize>> {
    let mut markdown = String::new();
    let mut offsets = vec![];
    for line in lines {
        offsets.push(markdown.len());
        markdown.push_str(&src[line.body..line.end]);
        markdown.push('\n');
    }
    let options = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut edits = vec![];
    let mut quotes = 0;
    let mut heading = false;
    for (event, range) in pulldown_cmark::Parser::new_ext(&markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::BlockQuote(_)) => quotes += 1,
            Event::End(TagEnd::BlockQuote(_)) => quotes -= 1,
            Event::Start(Tag::Heading { .. }) => heading = true,
            Event::End(TagEnd::Heading(_)) => heading = false,
            Event::SoftBreak if !heading => {
                let i = offsets.partition_point(|&offset| offset <= range.start) - 1;
                let line = &lines[i];
                let next = &lines[i + 1];
                let start = line.body + src[line.body..line.end].trim_end_matches([' ', '\t']).len();
                let mut content = &src[next.body..next.end];
                for _ in 0..quotes {
                    content = content.trim_start_matches([' ', '\t']);
                    let Some(rest) = content.strip_prefix('>') else { break };
                    content = rest;
                }
                content = content.trim_start_matches([' ', '\t']);
                edits.push(start..next.end - content.len());
            }
            _ => {}
        }
    }
    edits
}

/// Join Markdown prose in consecutive standalone comments with matching indentation and markers.
fn join_comment_lines(src: &str) -> String {
    let tree = parse(src);
    let mut comments = vec![];
    walk(tree.root_node(), |node| {
        if node.kind() != "line_comment" { return node.kind() != "block_comment"; }
        let start = line_start(src, node.start_byte());
        if !src[start..node.start_byte()].trim().is_empty() { return false; }
        let end = line_end(src, node.start_byte());
        let end = end - usize::from(src[..end].ends_with('\r'));
        let marker = if node.child_by_field_name("outer").is_some() || node.child_by_field_name("inner").is_some() { 3 } else { 2 };
        let prefix_end = node.start_byte() + marker;
        let body = prefix_end + usize::from(src[prefix_end..end].starts_with(' '));
        comments.push(CommentLine { start, body, end, prefix: &src[start..prefix_end] });
        false
    });
    comments.sort_by_key(|line| line.start);
    let mut edits = vec![];
    let mut start = 0;
    while start < comments.len() {
        let mut end = start + 1;
        while end < comments.len() && comments[end].prefix == comments[start].prefix
            && matches!(&src[comments[end - 1].end..comments[end].start], "\n" | "\r\n") { end += 1; }
        if end - start > 1 { edits.extend(comment_joins(src, &comments[start..end])); }
        start = end;
    }
    apply(src, edits.into_iter().map(|range| (range, " ".to_string())).collect())
}

/// Format `src` with rustfmt (stdin mode, sharing our width cap), then compact.
fn fastfmt(src: &str, edition: &str, width: usize, join_comments: bool) -> Result<String, String> {
    let mut child = Command::new("rustfmt").args([
        "--edition", edition, "--emit", "stdout", "--config",
        &format!("max_width={width},use_small_heuristics=Max,use_field_init_shorthand=true,disable_all_formatting=false"),
    ]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("failed to run rustfmt: {e}"))?;
    child.stdin.take().unwrap().write_all(src.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into_owned()); }
    let out = compact(&String::from_utf8_lossy(&out.stdout), width);
    Ok(if join_comments { join_comment_lines(&out) } else { out })
}

/// The value at `key` in the nearest config file at or above `path`.
fn toml_lookup(path: &Path, file: &str, key: &str) -> Option<String> {
    for dir in path.canonicalize().unwrap_or_else(|_| path.to_path_buf()).ancestors() {
        if let Ok(t) = std::fs::read_to_string(dir.join(file)) {
            return t.lines().find(|l| l.trim_start().starts_with(key)).and_then(|l| l.split('=').nth(1)).map(|v| v.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[derive(Clone)]
struct Config { edition: String, join_comments: bool }

impl Default for Config { fn default() -> Self { Self { edition: "2021".into(), join_comments: false } } }

fn join_comments_setting(metadata: &serde_json::Value) -> Result<Option<bool>, String> {
    match metadata.get("fastfmt").and_then(|settings| settings.get("join_comments")) {
        None => Ok(None), Some(value) => value.as_bool().map(Some).ok_or("fastfmt.join_comments must be a boolean".into())
    }
}

/// The nearest directory at or above `path` that holds a `Cargo.toml`.
fn package_dir(path: &Path) -> Option<&Path> { path.ancestors().find(|p| p.join("Cargo.toml").is_file()) }

/// Let Cargo resolve editions and metadata, caching every package in its response.
fn config_for(path: &Path, configs: &mut HashMap<PathBuf, Config>) -> Result<Config, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let Some(dir) = package_dir(&path) else { return Ok(Config::default()) };
    let manifest = dir.join("Cargo.toml");
    if let Some(config) = configs.get(&manifest) { return Ok(config.clone()); }
    let out = Command::new("cargo").args(["metadata", "--no-deps", "--offline", "--format-version", "1", "--manifest-path"]).arg(&manifest).output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).into_owned()); }
    let metadata: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("invalid cargo metadata: {e}"))?;
    let workspace_join_comments = join_comments_setting(&metadata["metadata"])?.unwrap_or(false);
    let packages = metadata["packages"].as_array().ok_or("cargo metadata has no packages")?;
    for package in packages {
        let manifest = package["manifest_path"].as_str().ok_or("cargo package has no manifest_path")?;
        let edition = package["edition"].as_str().ok_or("cargo package has no edition")?;
        let join_comments = join_comments_setting(&package["metadata"])?.unwrap_or(workspace_join_comments);
        let manifest = Path::new(manifest).canonicalize().map_err(|e| e.to_string())?;
        configs.insert(manifest, Config { edition: edition.into(), join_comments });
    }
    // A virtual workspace manifest owns no package: this is a standalone file.
    Ok(configs.entry(manifest).or_insert(Config { join_comments: workspace_join_comments, ..Config::default() }).clone())
}

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

/// The `rustfmt.toml` that rustfmt reads for `file`'s package, searching up to the repository root, or a new one in the package directory.
fn config_path(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap();
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let package = package_dir(&dir).unwrap_or(&dir);
    let repo = package.ancestors().find(|p| p.join(".git").exists()).unwrap_or(package);
    let mut configs = package.ancestors().take_while(|p| p.starts_with(repo)).map(|p| p.join("rustfmt.toml"));
    configs.find(|p| p.is_file()).unwrap_or_else(|| package.join("rustfmt.toml"))
}

/// Ensure ordinary rustfmt is disabled for each target project. Returns paths which needed an update; in check mode they are reported but left untouched.
fn guard_rustfmt(paths: &[PathBuf], check: bool) -> Result<Vec<PathBuf>, String> {
    let mut configs: Vec<_> = paths.iter().map(|p| config_path(p)).collect();
    configs.sort();
    configs.dedup();
    let mut dirty = vec![];
    for path in configs {
        if ignored(&path)? { continue; }
        let old = match std::fs::read_to_string(&path) {
            Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(), Err(e) => return Err(format!("{}: {e}", path.display()))
        };
        let new = guarded_config(&old);
        if new == old { continue; }
        dirty.push(path.clone());
        if !check { std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))? }
    }
    Ok(dirty)
}

fn ignore_root(path: &Path) -> &Path { path.ancestors().find(|p| p.join(".git").exists()).unwrap_or_else(|| path.ancestors().last().unwrap()) }

/// Walk roots bypass ignore rules, so check explicit targets and prospective configs separately.
fn ignored(path: &Path) -> Result<bool, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut matcher = WalkBuilder::new(ignore_root(&cwd)).build_matchers().remove(0);
    let relative = if let Some(relative) = matcher.normalize(path) { relative } else {
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        matcher = WalkBuilder::new(ignore_root(&path)).build_matchers().remove(0);
        matcher.normalize(&path).ok_or_else(|| format!("invalid path: {}", path.display()))?
    };
    if relative.components().any(|part| part.as_os_str() == "target") { return Ok(true); }
    let (matched, error) = matcher.matched_with_errors(relative, path.is_dir());
    if let Some(error) = error { return Err(error.to_string()); }
    Ok(matched.is_ignore())
}

fn rs_files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if ignored(path)? { return Ok(()); }
    for entry in WalkBuilder::new(path).filter_entry(|entry| entry.file_name() != "target").build() {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) && entry.path().extension().is_some_and(|e| e == "rs") { out.push(entry.into_path()); }
    }
    Ok(())
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "fastfmt") { args.remove(0); } // invoked as `cargo fastfmt`
    let check = args.iter().any(|a| a == "--check");
    let width_flag = args.iter().position(|a| a == "--width").and_then(|i| args.get(i + 1)).and_then(|w| w.parse().ok());
    let mut paths: Vec<PathBuf> = vec![];
    let mut skip = false;
    for (i, a) in args.iter().enumerate() {
        if skip { skip = false; continue; }
        match a.as_str() { "--check" => {} "--width" => skip = true, _ if i > 0 && args[i - 1] == "--width" => {} p => paths.push(p.into()) }
    }
    if paths.is_empty() { paths.push(".".into()) }
    let mut files = vec![];
    for path in &paths { if let Err(e) = rs_files(path, &mut files) { eprintln!("{e}"); std::process::exit(2) } }
    files.sort();
    files.dedup();
    let dirty_configs = match guard_rustfmt(&files, check) { Ok(paths) => paths, Err(e) => { eprintln!("{e}"); std::process::exit(2) } };
    let mut dirty = vec![];
    let mut configs = HashMap::new();
    for f in &files {
        let src = match std::fs::read_to_string(f) { Ok(s) => s, Err(e) => { eprintln!("{}: {e}", f.display()); std::process::exit(2) } };
        match config_for(f, &mut configs).and_then(|config| fastfmt(&src, &config.edition, width_for(f, width_flag), config.join_comments)) {
            Ok(new) if new != src => {
                if check { dirty.push(f) } else if let Err(e) = std::fs::write(f, &new) { eprintln!("{}: {e}", f.display()); std::process::exit(2) }
            }
            Ok(_) => {}
            Err(e) => { eprintln!("{}: {e}", f.display()); std::process::exit(2) }
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
    use super::{compact, fastfmt, guarded_config, join_comment_lines};

    /// Format `src` at `width`, asserting that a second pass changes nothing.
    fn stable(src: &str, width: usize) -> String {
        let out = fastfmt(src, "2021", width, false).unwrap();
        if out != src { assert_eq!(fastfmt(&out, "2021", width, false).unwrap(), out); }
        out
    }

    #[test]
    fn joins_markdown_prose() {
        let cases = [
            ("// One line\n// continues here.\n", "// One line continues here.\n"),
            ("/// One line\n/// continues here.\nfn f() {}\n", "/// One line continues here.\nfn f() {}\n"),
            ("//! One line\n//! continues here.\n", "//! One line continues here.\n"),
            ("fn f() {\n    // café\n    // 日本語\n}\n", "fn f() {\n    // café 日本語\n}\n"), ("//one\r\n//two", "//one two"),
            ("// - First item\n//   continues.\n// - Second item\n", "// - First item continues.\n// - Second item\n"),
            ("// 3. First item\n//    continues.\n// 4. Second item\n", "// 3. First item continues.\n// 4. Second item\n"),
            ("// > - First item\n// >   continues.\n// > - Second item\n", "// > - First item continues.\n// > - Second item\n"),
            ("// > > Nested quote\n// > > continues.\n", "// > > Nested quote continues.\n"),
            ("// > Lazy quote\n// continues with \\*literal markup.\n", "// > Lazy quote continues with \\*literal markup.\n"),
            ("// Before `some code`\n// and [a link](https://example.com).\n", "// Before `some code` and [a link](https://example.com).\n"),
            ("// First paragraph.\n//\n// Second paragraph\n// continues.\n", "// First paragraph.\n//\n// Second paragraph continues.\n"),
            ("// [^n]: A footnote\n//     continues here.\n", "// [^n]: A footnote continues here.\n"),
        ];
        for (src, want) in cases {
            let out = join_comment_lines(src);
            assert_eq!(out, want, "{src}");
            assert_eq!(join_comment_lines(&out), out);
        }
        let src = "/// A paragraph that already exceeds the width\n/// continues without wrapping.\nfn f() {}\n";
        let out = fastfmt(src, "2021", 40, true).unwrap();
        assert!(out.contains("/// A paragraph that already exceeds the width continues without wrapping."));
        assert_eq!(fastfmt(&out, "2021", 40, true).unwrap(), out);
    }

    #[test]
    fn preserves_comment_structure() {
        for src in [
            "// ```rust\n// let x = 1;\n// let y = 2;\n// ```\n", "//     let x = 1;\n//     let y = 2;\n", "// - input -> parser\n// - parser -> output\n",
            "// # Heading\n//\n// Multiline heading\n// stays unchanged\n// =================\n", "// | A | B |\n// |---|---|\n// | 1 | 2 |\n",
            "// First line  \n// second line\\\n// third line\n", "// `inline code\n// with a newline`\n",
            "// <pre>\n// first line\n// second line\n// </pre>\n", "// First paragraph\n\n// Second paragraph\n",
            "// Ordinary comment\n/// Doc comment\nfn f() {}\n", "// Outer indentation\n    // Different indentation\n",
            "fn f() {\n    drop(1); // Trailing comment\n    // Standalone comment\n}\n",
            "fn f() { let s = r#\"\n// Not a comment\n// Still not a comment\n\"#; }\n",
            "/*\n// Inside a block comment\n// Still inside the block comment\n*/\n",
        ] { assert_eq!(join_comment_lines(src), src, "{src}"); }
    }

    #[test]
    fn joins_statement_with_tail() {
        for body in [
            "x = step(x);\nSome(x)", "let y = compute(x);\ny.finish()", "a();\n42", "a();\nif c { b() } else { d() }", "eprintln!(\"x\");\nreturn;",
            "while a { b(); }\nbreak;",
        ] {
            let src = format!("fn f() {{\n{body}\n}}\n");
            assert_eq!(compact(&src, 150), format!("fn f() {{ {} }}\n", body.replace('\n', " ")));
        }
        for exit in ["break", "continue", "return"] {
            let src = format!(
                "fn f() {{\n    loop {{\n        if cancelled {{\n            error = Some(e);\n            {exit};\n        }}\n        work();\n    }}\n}}\n"
            );
            let out = stable(&src, 160);
            assert!(out.contains(&format!("if cancelled {{ error = Some(e); {exit}; }}")), "{out}");
        }
        let src = "fn f() { before(); let steps = |mut x: f64, step: fn(f64) -> f64| std::iter::from_fn(move || { x = step(x); Some(x) }).take(64); }\n";
        let out = stable(src, 160);
        assert!(out.contains("std::iter::from_fn(move || { x = step(x); Some(x) }).take(64)"), "{out}");
    }

    #[test]
    fn formats_value_chains() {
        let src = "fn f() {\n    let value = if a { one() }\n    else if b {\n        work();\n        more();\n        two()\n    } else { three() };\n    done(value);\n}\n";
        assert_eq!(stable(src, 160), src);
        let src = "fn f() { if a { one() } else if b { two() } else { three() } }\n";
        assert_eq!(stable(src, 160), src);
        let out = stable(src, 40);
        assert!(out.contains("\n    else if b") && out.contains("\n    else { three() }"), "{out}");
        for src in [
            "fn f() {\n    let value = if ready { one() } else {\n        work();\n        more();\n        two()\n    };\n    done(value);\n}\n",
            "fn f() {\n    let value = if ready {\n        work();\n        more();\n        two()\n    } else { three() };\n    done(value);\n}\n",
            "fn f() {\n    if ready {\n        work();\n        more();\n        two();\n    } else {\n        work();\n        more();\n        three();\n    }\n    done();\n}\n",
            "fn f() {\n    if ready { one() } else {\n        work();\n        more();\n        two();\n    }\n    done();\n}\n",
        ] {
            assert_eq!(stable(src, 160), src);
            assert_eq!(compact(&src.replace("} else {", "}\n    else {"), 160), src);
        }
    }

    #[test]
    fn compacts_call_scaffolding() {
        for body in [
            "x.try_iter()?.enumerate().map(|(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        rational(&x?, &fraction)\n    }).collect()",
            "let xs = xs.into_iter().enumerate().map(|(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        fraction.call1((x.numer().clone(), x.denom().clone()))\n    }).collect::<PyResult<Vec<_>>>()?;",
            "output(py, x.iter().zip(&y).enumerate().map(|(i, (x, y))| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        f(x, y)\n    }).collect::<PyResult<_>>()?)",
            "output(a_rather_long_argument_to_the_outer_call, &mut |(i, x)| {\n        if i % 1024 == 0 { py.check_signals()?; }\n        rational(&x?, &fraction)\n    })",
        ] { let src = format!("fn f() {{\n    {body}\n}}\n"); for width in [120, 160] { assert_eq!(stable(&src, width), src); } }
        let src = "fn f() {\n    output(py, xs.map(|x| {\n        let y = a(x);\n        b(y);\n        y\n    }).collect::<Result<Vec<_>, E>>()?)\n}\n";
        for width in [40, 55, 80] {
            let out = stable(src, width);
            assert!(out.lines().all(|line| line.len() <= width), "{out}");
            assert!(out.contains("let y = a(x);\n"));
            if width >= 55 { assert!(out.contains("}).collect::<Result<Vec<_>, E>>()?)"), "{out}"); }
        }
        for body in ["// keep this comment\n        work(x)", "let s = r#\"first\nsecond\"#;\n        work(s)"] {
            let src = format!("fn f() {{\n    output(xs.map(|x| {{\n        {body}\n    }}).collect())\n}}\n");
            assert_eq!(compact(&src, 80), src);
            stable(&src, 80);
        }
        let src =
            "fn f() {\n    output(\n        xs.map(|x| {\n            let s = r#\"first\nsecond\"#;\n            work(s)\n        }).collect(),\n    )\n}\n";
        assert_eq!(compact(src, 160), src); // moving the callback left must not reindent its literal
        let src = "fn f() {\n    let pair = (name, json!({\n        \"data\": event.get(\"data\").cloned().unwrap_or_else(|| json!({})),\n        \"metadata\": event.get(\"metadata\").cloned().unwrap_or_else(|| json!({})),\n    }));\n    consume(pair);\n}\n";
        assert_eq!(stable(src, 100), src);
    }

    #[test]
    fn joins_nested_imports() {
        let want = "use rustymath::{Arg, Unary, distribution::{self as dist, standard::*}};\n";
        for width in [want.len() - 1, want.len() - 2] {
            let out = fastfmt(want, "2024", width, false).unwrap();
            assert_eq!(out == want, width == want.len() - 1, "{out}");
            assert_eq!(fastfmt(&out, "2024", width, false).unwrap(), out);
        }
        let src = "fn f() {\n    g(\n        (1,),\n        (1, 2,),\n        \"comma, }\",\n    )\n}\n";
        assert!(compact(src, 100).contains("g((1,), (1, 2), \"comma, }\")"));
    }

    #[test]
    fn sizes_bodies() {
        for line in [
            "fn gcd<'py>(py: Python<'py>, x: &Bound<'py, PyAny>, y: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> { binary(py, x, y, |x, y| Ok(exact::gcd(x, y))) }",
            "fn pdf<'py>(&self, py: Python<'py>, x: PyArrayLikeDyn<'py, f64, AllowTypeChange>) -> Bound<'py, PyAny> { each(py, x, dist::Pdf(self.0)) }",
            "fn cdf<'py>(&self, py: Python<'py>, x: PyArrayLikeDyn<'py, f64, AllowTypeChange>) -> Bound<'py, PyAny> { each(py, x, dist::Cdf(self.0)) }",
        ] {
            let src = format!("{line}\n");
            assert_eq!(stable(&src, 160), src);
        }
        for (len, width, join) in [(120, 160, true), (121, 160, false), (120, 119, false)] {
            let want = format!("fn f() {{ {}() }}\n", "a".repeat(len - 6));
            let out = stable(&want, width);
            assert_eq!(out == want, join, "{out}");
        }
        let src = format!("fn f() {{\n    if c {{\n        {}();\n    }}\n    done();\n}}\n", "x".repeat(115));
        assert_eq!(compact(&src, 160), src); // an if body has the same 120-character budget
    }

    #[test]
    fn joins_simple_macro_arms() {
        let src = "macro_rules! floats {\n    ($a:expr, $t:ident => $body:expr) => {\n        match $a.dtype().char() {\n            b'e' => {\n                type $t = f16;\n                $body\n            }\n            b'f' => {\n                type $t = f32;\n                $body\n            }\n            _ => {\n                type $t = f64;\n                $body\n            }\n        }\n    };\n}\n";
        let out = stable(src, 160);
        for (arm, ty) in [("b'e'", "f16"), ("b'f'", "f32"), ("_", "f64")] { assert!(out.contains(&format!("{arm} => {{ type $t = {ty}; $body }}")), "{out}"); }
        for body in ["let a = 1;\nlet b = 2;\n$body", "if ready { work() }\n$body", "$($x;)*", "// Keep this comment\n$body", "r#\"first\nsecond\"#"] {
            let src = format!("macro_rules! m {{ () => {{ match x {{ _ => {{\n{body}\n}} }} }}; }}\n");
            assert_eq!(compact(&src, 160), src);
        }
    }

    #[test]
    fn packs_continuations_and_lists() {
        let src = r#"struct Cell<'a> { layout: &'a MatrixLoop, pointers: &'a [usize], outer: &'a [npy_intp], core: &'a [npy_intp], dims: &'a [usize], i: usize }
enum Kind { E, R, C, I }
enum Tall<'a, T> { Q, #[cfg(native)] R, Project(&'a [T], usize) }
trait Cells {
    type Shared;
    type Per;
    type Cell<'a> where Self: 'a;
    unsafe fn cell<'a>(shared: &'a Self::Shared, per: Self::Per, i: usize) -> Self::Cell<'a> where Self: 'a;
}
fn entry<E: Item>(f: Ufunc, kinds: &[Kind]) -> Loop { (Some(f), kinds.iter().map(|k| (match k { Kind::E => E::CODE, Kind::R => E::Real::CODE, Kind::C => E::Complex::CODE, Kind::I => i64::CODE }) as c_char).collect()) }
impl<E: Item> Gufunc<E> for Permanent where E::W: NumAssignRef {
    type S = ();
    fn each() { work(); more(); done(); }
}
unsafe fn folded<K: Kernel<E, N>, E: Code, const N: usize>(k: &K, n: usize, x: &[*mut c_char], xs: &[npy_intp], o: *mut c_char, os: npy_intp, reducing: bool) -> bool {
    work(); more(); true
}
unsafe extern "C" fn inner<M: Make, E: Code, const N: usize>(args: *mut *mut c_char, dims: *mut npy_intp, steps: *mut npy_intp, _: *mut c_void)
where M::K: Kernel<E, N>, {
    let n = unsafe { *dims } as usize;
    if n == 0 { return; }
    work(n)
}
fn f() {
    let transposed = Self { op: match self.op { Op::Normal => Op::Transpose, Op::Transpose => Op::Normal, Op::Adjoint => Op::Conjugate, Op::Conjugate => Op::Adjoint }, ..self };
    register(m, "minmax", "The minimum and maximum along an axis, preserving the input dtype. Empty lanes give the dtype's extrema identities.", [1, 2], NONE, Some("(n)->(),()"), vec![minmax_entry::<bool>(), minmax_entry::<i8>(), minmax_entry::<u8>(), minmax_entry::<i16>(), minmax_entry::<u16>(), minmax_entry::<i32>(), minmax_entry::<u32>(), minmax_entry::<i64>(), minmax_entry::<u64>(), minmax_entry::<f16>(), minmax_entry::<f32>(), minmax_entry::<f64>()], None)?;
    let complex = vec![entry::<Complex<f32>>(looped::<Product<PlusTimes>, Complex<f32>>, &complex), entry::<Complex<f64>>(looped::<Product<PlusTimes>, Complex<f64>>, &complex)];
    register(m, "matmul", "The matrix product of each pair of matrices, or for Booleans, whether some k has both a[i,k] and b[k,j]. Integers wrap on overflow.", [2, 1], PRODUCT, [vec![or_and, integer_product::<PlusTimes>()], float_products::<PlusTimes>(), complex].concat())?;
    register(m, "maximum_add", "The max-plus product of each pair of real matrices: the greatest a[i,k] + b[k,j].", [2, 1], PRODUCT, [vec![integer_product::<MaxPlus>()], float_products::<MaxPlus>()].concat())?;
    register(m, "minimum_add", "The min-plus product of each pair of real matrices: the least a[i,k] + b[k,j].", [2, 1], PRODUCT, [vec![integer_product::<MinPlus>()], float_products::<MinPlus>()].concat())?;
}
"#;
        let out = stable(src, 160);
        assert!(out.lines().all(|line| line.len() <= 160), "{out}");
        assert!(out.contains(src.lines().next().unwrap()), "{out}");
        assert!(out.contains("enum Kind { E, R, C, I }"), "{out}");
        assert!(out.contains("enum Tall<'a, T> { Q, #[cfg(native)] R, Project(&'a [T], usize) }"), "{out}");
        assert!(out.contains("    unsafe fn cell<'a>(shared: &'a Self::Shared, per: Self::Per, i: usize) -> Self::Cell<'a> where Self: 'a;"), "{out}");
        assert!(out.contains("match k { Kind::E => E::CODE, Kind::R => E::Real::CODE, Kind::C => E::Complex::CODE, Kind::I => i64::CODE }"), "{out}");
        assert!(out.contains("}).collect())"), "{out}");
        assert!(out.contains("impl<E: Item> Gufunc<E> for Permanent where E::W: NumAssignRef {"), "{out}");
        assert!(out.contains("(\n    k: &K, n: usize, x: &[*mut c_char], xs: &[npy_intp]"), "{out}");
        assert!(out.contains("reducing: bool) -> bool {"), "{out}");
        assert!(out.contains("}, ..self }"), "{out}");
        assert!(out.contains("\n    let n = unsafe { *dims } as usize;\n"), "{out}");
        assert!(out.contains("minmax_entry::<bool>(), minmax_entry::<i8>()"), "{out}");
        assert!(out.contains("], None)?;"), "{out}");
        assert!(out.contains("[2, 1], PRODUCT,"), "{out}");
        // A parenthesised macro group packs inside its outer lines.
        let src = "fn f() -> bool {\n    matches!(\n        kind,\n        \"a\"\n            | \"b\"\n    )\n}\n";
        assert_eq!(compact(src, 160), "fn f() -> bool {\n    matches!(\n        kind, \"a\" | \"b\"\n    )\n}\n");
    }

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
            ("fn f(x: u8) -> u8 {\n    if x > 1 {\n        return 1;\n    }\n    0\n}\n", "fn f(x: u8) -> u8 { if x > 1 { return 1; } 0 }\n"),
            ("fn f(ready: bool) {\n    if ready {\n        notify();\n    }\n}\n", "fn f(ready: bool) { if ready { notify(); } }\n"), (
                "fn f(fds: Fds) {\n    for fd in fds {\n        let _ = self.poller.delete(borrowed(fd));\n    }\n}\n",
                "fn f(fds: Fds) { for fd in fds { let _ = self.poller.delete(borrowed(fd)); } }\n"),
            ("fn time(&self) -> f64 {\n    self.core.time()\n}\n", "fn time(&self) -> f64 { self.core.time() }\n"), (
                "fn f(c: bool, e: &mut E, h: H) {\n    if c {\n        e.writer = Some(h)\n    } else {\n        e.reader = Some(h)\n    }\n}\n",
                "fn f(c: bool, e: &mut E, h: H) { if c { e.writer = Some(h) } else { e.reader = Some(h) } }\n"), (
                "fn f(c: bool) {\n    if c {\n        a()\n    } else {\n        b()\n    }\n    done();\n}\n",
                "fn f(c: bool) {\n    if c { a() }\n    else { b() }\n    done();\n}\n"),
            ("struct FdEntry<H> {\n    reader: Option<H>,\n    writer: Option<H>,\n}\n", "struct FdEntry<H> { reader: Option<H>, writer: Option<H> }\n"),
            ("enum Rt {\n    Owned(Runtime),\n    Borrowed(Handle),\n}\n", "enum Rt { Owned(Runtime), Borrowed(Handle) }\n"), (
                "fn drain(&self, q: &mut Q) {\n    while let Some(h) = q.pop_front() {\n        ready.push_back(h)\n    }\n}\n",
                "fn drain(&self, q: &mut Q) { while let Some(h) = q.pop_front() { ready.push_back(h) } }\n"), (
                "impl<H> Default for FdEntry<H> {\n    fn default() -> Self {\n        Self { reader: None, writer: None }\n    }\n}\n",
                "impl<H> Default for FdEntry<H> { fn default() -> Self { Self { reader: None, writer: None } } }\n"), (
                "fn h(&self) -> Handle {\n    match self {\n        Rt::Owned(r) => r.handle().clone(),\n        Rt::Borrowed(h) => h.clone(),\n    }\n}\n",
                "fn h(&self) -> Handle { match self { Rt::Owned(r) => r.handle().clone(), Rt::Borrowed(h) => h.clone() } }\n"),
        ];
        for (src, want) in cases { assert_eq!(compact(src, 150), want, "src: {src}") }
    }

    #[test]
    fn keeps_what_must_stay() {
        for src in [
            "fn f() {\n    a();\n    b();\n    c()\n}\n",                                          // two statements plus a tail
            "fn f() {\n    a();\n    fn g() {}\n}\n",                                              // an item is not a tail expression
            "fn f() {\n    a();\n    b();\n}\n",                                                   // two statements in a fn body
            "fn f(ready: bool) {\n    if ready {\n        notify();\n        record();\n        return;\n    }\n}\n", // two actions plus an exit stay multiline
            "fn f(r: R) -> u8 {\n    match r {\n        Err(e) => {\n            log(e);\n            record(e);\n            return 1;\n        }\n        Ok(v) => v,\n    }\n}\n", // multi-statement arm: neither the match nor an enclosing block joins
            "fn f(a: bool) -> u8 {\n    if a { b(); }\n    else { c(); }\n    x\n}\n",   // a statement-position else keeps its own line
            "fn f() {\n    m![a, |x| {\n        // keep\n        work(x);\n        more(x)\n    }];\n}\n", // a closure body that crosses a macro list's last line keeps its comment and statements
            "fn f() {\n    // why\n    a()\n}\n",   // comment must survive
            "fn f() {\n    let s = \"a\nb\";\n}\n", // multiline string literal
        ] { assert_eq!(compact(src, 150), src) }
        let long = format!("fn f() {{\n    {}()\n}}\n", "x".repeat(160));
        assert_eq!(compact(&long, 150), long); // width cap
    }
}
