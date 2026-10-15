# Development

## Design

Everything lives in `src/main.rs`. `fastfmt()` pipes a file through `rustfmt --emit stdout` (width and `use_small_heuristics=Max` passed via `--config`, so the project's `disable_all_formatting` guard is overridden), then `compact()` re-joins. `guard_rustfmt()` creates or updates that guard before formatting. Joining is syntax-aware via tree-sitter: `join_limit` names the joinable node kinds and their item limits, `joined` renders a candidate onto one line (dropping a trailing comma, adding no space before `.`-led continuations), and `compact_round` applies innermost candidates first, iterating to a fixpoint so nested one-liners collapse. Anything containing a comment or a multi-line token never joins. Code blocks join with at most one statement, optionally followed by a tail expression, and semicolons are preserved exactly. After joining, statement-position `else` clauses move to their own line; value-position `if`/`else` expressions are unchanged.

The tuning benchmark is AnswerDotAI/loopmini: run stable `cargo fmt` on its `src/`, then `cargo-fastfmt`, and diff against the original. Joined one-liners use the same width cap as rustfmt; the remaining diff should be only rustfmt canonicalisations such as import order and trailing semicolons.

## Commands

Edition and comment-setting lookup uses offline `cargo metadata --no-deps`, caching the resolved editions and settings for all workspace packages. This respects workspace inheritance, member overrides, and Cargo's default edition without reimplementing manifest parsing. `package.metadata.fastfmt.join_comments` overrides `workspace.metadata.fastfmt.join_comments`, defaulting to false. Invalid non-boolean values are errors.
Standalone Rust files outside a Cargo package, including files under a virtual workspace root, retain the 2021 fallback.

When enabled, `join_comment_lines()` runs after code compaction. Tree-sitter finds standalone line comments; consecutive comments with identical indentation and markers form a Markdown document. pulldown-cmark identifies soft breaks, which map back to source edits removing only the break, comment prefix, and Markdown continuation prefix. Other source text is not serialized or normalized. The code-width limit does not apply to comment paragraphs.

```bash
maturin develop && pytest -q
ship-rs-build
```

## Versioning

The canonical version lives in `Cargo.toml`. `pyproject.toml` gets the Python package version from Cargo via `dynamic = ["version"]`.

## Release

1. Run `maturin develop && pytest -q`.
2. Confirm the release version in `Cargo.toml` (`[package].version`).
3. Run `ship-release`.

Fastship commits the changelog, pushes the version tag for GitHub Actions, then bumps and pushes `Cargo.toml`.
