# Development

## Design

Everything lives in `src/main.rs`. `fastfmt()` pipes a file through `rustfmt --emit stdout` (width, `use_small_heuristics=Max` and `use_field_init_shorthand=true` passed via `--config`, so the project's `disable_all_formatting` guard is overridden), then `compact()` re-joins. `guard_rustfmt()` creates or updates that guard before formatting. Tree-sitter supplies syntax boundaries. `shape` decides how a node may compact: a `Body` joins whole, a `List` packs inside its first and last lines, and a `Run` packs throughout. `compacted` renders one candidate. `compact_round` applies nonoverlapping candidates top-down, repeating until stable. Each accepted edit removes a source newline, so convergence does not need a round limit. After compaction, `format_elses` places `else` branches. README's "Joining rules" section states the rules these functions implement.

The tuning benchmark is AnswerDotAI/loopmini: run stable `cargo fmt` on its `src/`, then `cargo-fastfmt`, and diff against the original. `COMPACT_WIDTH` (120) is the body budget, and also the budget for the call or chain enclosing a joined closure body.

One `Layout` scan collects preserved multiline islands, protected comments/tokens, optional trailing commas and existing argument/parameter opening breaks. Its shared renderer removes only syntactic optional commas, never commas inside literals, singleton tuples or opaque macro groups. `pack_lines` greedily erases existing breaks, never creating new breakpoints. Long lists keep their outer breaks; long argument/parameter runs keep their opening break and pack continuation lines. A list doesn't pack when an island crosses its first or last line. Multiline islands align to the packed prefix, except declaration bodies keep their existing indent. Comments/multiline tokens outside islands prevent packing; protected islands retain their bytes and cannot be reindented.

Tree-sitter leaves macro expansions opaque. `simple_macro_body` permits whitespace-only joining of tiny `=> { ... }` groups with at most one direct semicolon and no nested token groups or repetition. Parenthesised and bracketed macro groups that contain commas are lists, which pack existing interior lines without changing tokens. Brace groups remain boundaries, because they usually hold statements or items. There is no macro substitution, reparse or serializer.

## Commands

Dev builds retain debug information and use `opt-level = 2`, so the locally installed formatter stays fast enough for real projects.

File discovery uses `ignore::WalkBuilder`, retaining its standard ignore filters and excluding `target/`. Because explicitly supplied walk roots bypass ignore rules, an incremental matcher rooted at the current Git repository checks explicit targets first; paths outside that repository use their own enclosing repository. This also protects ignored nested repositories. The same check protects prospective `rustfmt.toml` paths, including ones that do not exist yet. Guards are derived from discovered source files, not unfiltered command-line paths.

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
