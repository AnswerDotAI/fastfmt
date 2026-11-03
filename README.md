# fastfmt

`cargo fmt`, then put the one-liners back.

Stable rustfmt expands constructs such as short function bodies, statement-position `if`/`else`, and short `match` or `struct` bodies. Some options for keeping them compact are nightly-only. Others have no rustfmt option. fastfmt runs rustfmt through stdin, then rejoins comment-free constructs within its size and width limits, giving fastai-style compact Rust from a stable toolchain.

## Install and use

```bash
pip install fastfmt
```

The wheel contains one `cargo-fastfmt` binary and no Python code. With the binary on `PATH`, Cargo recognizes it as a subcommand:

```bash
cargo fastfmt            # format the current directory tree in place
cargo fastfmt --check    # exit 1 listing files that would change; write nothing
cargo fastfmt src lib.rs # format specific files or directories
```

rustfmt must also be on `PATH`. Install it with `rustup component add rustfmt`.

Traversal respects `.gitignore`, `.git/info/exclude`, global Git ignores, and `.ignore` files, including for explicitly supplied paths. Hidden entries and `target/` directories are skipped. Ignored files, including `rustfmt.toml`, are never written.

`--width N` sets the hard line cap, defaulting to 160. Signatures, imports, lists and expression continuations pack up to that cap. Each joined body must also fit a budget of `min(120, N)` by itself, not counting the signature or condition before it. A closure body joins only when its enclosing call or chain also fits that budget.

fastfmt runs rustfmt with `use_small_heuristics = "Max"` and `use_field_init_shorthand = true`, which writes `Foo { x: x }` as `Foo { x }`. These settings override the project's `rustfmt.toml`.

On a normal run, fastfmt adds `disable_all_formatting = true` to the `rustfmt.toml` that rustfmt reads for each formatted package: the nearest one from the package directory up to the repository root. If there is none, fastfmt creates one in the package directory. This prevents `cargo fmt` or editor rustfmt runs from undoing the compact formatting. fastfmt overrides the setting for its own pass. No pre-existing config file is needed.

`--check` never writes the config. It reports a missing or disabled guard as a required update.

Adjacent `use` declarations with the same prefix and visibility merge into one group: three separate `use ::exhash::Name;` imports become `use ::exhash::{EditResult, FileEdit, FileError};`. Existing groups, aliases and globs are supported. Blank lines, comments, attributes and visibility differences remain boundaries; macro token trees are untouched. rustfmt orders and lays out the resulting group, and fastfmt packs it within the width cap.

## Shortening qualified paths

Find paths worth considering with a read-only report:

```bash
cargo fastfmt candidates          # scan the current tree
cargo fastfmt candidates src      # or select files/directories
cargo fastfmt candidates --json   # one machine-readable report with per-file counts
```

The report lists paths used at least twice in at least one file, sorted by total frequency. It includes occurrence and module counts, the maximum uses in one file (`max/file`), and per-file examples and counts. Type and trait paths use their first UpperCamelCase-looking segment: `std::time::Duration::from_millis` counts toward `std::time::Duration`.

Qualified free-function calls suggest their parent module. Calls to `std::thread::sleep` and `std::thread::spawn` both count toward `std::thread`; `std::mem::size_of::<T>()` counts toward `std::mem`. Modules already referred to by one name, such as `thread::sleep`, need no import and are not suggested. Function references outside calls are not counted. These are syntax and naming heuristics, not name resolution.

Existing imports, comments, strings, attributes, visibility restrictions and macro token trees are excluded. Obvious conflicts with imports/declarations and candidates sharing a short name are flagged, not hidden. The command does not run Cargo or rustfmt and never writes sources or configuration.

`--json` returns an array of candidates with `path`, `uses`, `modules`, `max_file`, `shared_name`, and `files`. Each file entry has `file`, `uses`, `modules`, `example`, and a `conflict` location or `null`. One invocation provides the whole batch; there is no need to scan each file separately. A shared short name is informational: different paths can be selected together when they are used in separate modules.

Use an explicit, opt-in refactoring mode to introduce imports and shorten selected paths. Create `imports.txt`:

```text
crate::float::Native
crate::recipe::NativeOps
num_complex::Complex
std::thread
```

```bash
cargo fastfmt shorten imports.txt --check  # preview affected files; write nothing
cargo fastfmt shorten imports.txt          # rewrite and format the current tree
cargo fastfmt shorten imports.txt src      # or select files/directories as usual
```

The file contains one qualified path per line; blank lines are ignored. Aliases, globs and generic arguments are not accepted in the list.

Each file root and inline module gets only the imports it needs. Existing equivalent imports, including grouped imports, are reused; shortening adds plain private `use` declarations and leaves their merging/layout to normal fastfmt formatting. Matching is syntax-aware: `crate::float::Native::new()` becomes `Native::new()`, `std::thread::sleep(...)` becomes `thread::sleep(...)`, and type arguments are preserved. Path replacements leave comments, strings, attributes, existing `use` declarations and macro token trees untouched. The normal formatting pass afterward merges adjacent imports as described above.

The user is responsible for choosing paths that are safe to shorten. One list may include different paths with the same short name, such as `crate::Result` in a core crate and `kernmini::Result` in its bindings. fastfmt rejects them only when both are used in the same module; conflicts with explicit imports or directly declared module items are also errors. Locations are reported without inventing aliases. It does not resolve glob imports, local/generic shadowing, conditional compilation, macro-generated names or changes to trait method lookup. A detected conflict or formatting error stops the command before any source files or rustfmt guards are written.

## Comment joining

Opt in to joining hard-wrapped comment paragraphs in `Cargo.toml`:

```toml
[package.metadata.fastfmt]
join_comments = true
```

Use `[workspace.metadata.fastfmt]` for workspace-wide defaults. Package settings override the workspace setting, including `join_comments = false`. Comment joining is off when neither sets it.

Consecutive standalone `//`, `///`, or `//!` comments with matching indentation and markers are treated as Markdown. Soft paragraph breaks become spaces, including wrapped continuations within list items and block quotes. Paragraphs can exceed `--width`; fastfmt does not rewrap them.

Lists, code blocks, tables, headings, inline code, raw HTML, and explicit Markdown hard breaks (a trailing backslash or two spaces) retain their structure. Use lists for separate points and code fences for diagrams, disabled code, or other layout-sensitive text. Unmarked line breaks within prose are joined. Trailing comments and block comments are not joined.

## Joining rules

A construct can join onto one line when it fits its budget and contains no comments or multiline tokens. The supported constructs are:

- Blocks, including function, closure, `if`/`else`, loop and match-arm bodies, with at most one statement, optionally followed by a tail expression or by `break`, `continue` or `return`. A block whose first statement is an `if`/`else` chain doesn't join.
- Single-item `impl` blocks.
- Struct fields, enum variants, simple match arms, nested `use` declarations and other comma-separated lists, limited by width rather than item count.

Semicolons are preserved exactly. Statement chains and multiline/over-width value chains separate `else if` and compact `else` branches after compact bodies. Multiline branch braces keep `} else {` attached. Value chains that fit entirely on one line stay inline.

Continuation packing erases existing breaks within expressions, parameter/argument lists and declaration headers while they fit the hard line cap. Multiline bodies and lists remain separate islands, with list interiors packed independently. This keeps meaningful closure bodies readable without expanding every method or enclosing parenthesis. The packer never invents breakpoints or splits tokens.

Inside opaque macro token trees, only tiny `=> { ... }` groups with at most one direct semicolon and no nested token groups join, and their tokens are never rewritten. Parenthesised and bracketed macro groups that contain commas pack their existing interior lines without changing tokens. Brace-delimited macro groups never pack.

Joins repeat so nested constructs can collapse fully. Comments and multiline strings are protected boundaries; packing is skipped when it would require altering their contents or indentation.
