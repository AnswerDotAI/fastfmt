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
