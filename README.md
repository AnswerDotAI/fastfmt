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

`--width N` sets the line cap for rustfmt and joined one-liners, which defaults to 160.

On a normal run, fastfmt creates or updates the target project's `rustfmt.toml` with `disable_all_formatting = true`. This prevents `cargo fmt` or editor rustfmt runs from undoing the compact formatting. fastfmt overrides the setting for its own pass. No pre-existing config file is needed.

`--check` never writes the config. It reports a missing or disabled guard as a required update.

## Joining rules

A construct can join onto one line when it fits the width cap and contains no comments or multiline tokens. The supported constructs are:

- Ordinary blocks, including function, closure, `if`/`else`, and loop bodies, with at most one statement, optionally followed by a tail expression.
- `match`, `struct`, and `enum` bodies with up to three arms, fields, or variants.
- Single-item `impl` blocks.

Semicolons are preserved exactly. A statement-position `else` starts on a new line. An `if`/`else` used as a value stays on one line.

Joins run innermost-first and repeat until nothing more can join. This allows nested constructs to collapse fully. Blocks used as match-arm bodies stay expanded. Blocks containing comments or multiline strings retain rustfmt's output.
