# fastfmt

`cargo fmt`, then put the one-liners back.

Stable rustfmt always breaks small constructs onto multiple lines: a
single-expression fn body, a statement-position if/else, a short match or
struct body. The options that would prevent this are nightly-only, and some
shapes have no option at all. fastfmt runs rustfmt (via stdin, so no config
file is needed), then re-joins those constructs when they are comment-free,
small, and within a width cap, giving fastai-style compact Rust from a stable
toolchain.

## Install

```bash
pip install fastfmt
```

The wheel ships a single `cargo-fastfmt` binary and no Python code. With it on
PATH, cargo picks it up as a subcommand:

```bash
cargo fastfmt            # format the current directory tree in place
cargo fastfmt --check    # exit 1 listing files that would change; write nothing
cargo fastfmt src lib.rs # format specific files or directories
```

`--width N` sets the rustfmt line cap (default 130). Joined one-liners use
tighter caps: 105 columns, or 80 for a two-statement block. rustfmt must be on
PATH (`rustup component add rustfmt`).

## What gets joined

A block joins onto one line only when nothing in it is a comment or spans
multiple lines itself, and the result fits the cap: fn bodies with one
statement, if/else and loop bodies with one or two, match/struct/enum bodies
with up to three arms, variants, or fields, and single-item impl blocks.
Joins run innermost-first to a fixpoint, so nested one-liners collapse fully.
Match arms stay expanded, and any block containing a comment or a multi-line
string is left exactly as rustfmt wrote it.
