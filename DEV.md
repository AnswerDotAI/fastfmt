# Development

## Design

Everything lives in `src/main.rs`. `fastfmt()` pipes a file through `rustfmt
--emit stdout` (width and `use_small_heuristics=Max` passed via `--config`, so
target crates need no rustfmt.toml), then `compact()` re-joins. Joining is
syntax-aware via tree-sitter: `join_limit` names the joinable node kinds and
their item limits, `joined` renders a candidate onto one line (dropping a
trailing comma, adding no space before `.`-led continuations), and
`compact_round` applies innermost candidates first, iterating to a fixpoint so
nested one-liners collapse. Anything containing a comment or a multi-line
token never joins.

The tuning benchmark is AnswerDotAI/loopmini: run stable `cargo fmt` on its
`src/`, then `cargo-fastfmt`, and diff against the original. The join caps
(105, and 80 for two-statement blocks) come from that exercise; the remaining
diff should be only rustfmt canonicalisations such as import order and
trailing semicolons.

## Commands

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
