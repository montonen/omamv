# omamv

Omarchy Markdown Viewer: a read-only, keyboard-first viewer for a folder of
Markdown. Rust (edition 2024), GTK4 + libadwaita for the window, the document
as a local WebKitGTK 6 page served from `omamv://` under a strict CSP.
pulldown-cmark + ammonia render it, syntect highlights code, Mermaid is
bundled in `assets/`, PlantUML runs locally in its sandbox. `src/ui/` is the
GTK side; `src/*.rs` holds no GTK except `main.rs`, `scheme.rs` and `theme.rs`.

## Commands

```bash
cargo test                      # unit tests
cargo clippy --all-targets -- -D warnings   # as CI runs it
scripts/check.sh                # the app offscreen on Broadway, every key path, then smoke.sh
scripts/smoke.sh                # the real binary, started as a person would
scripts/install.sh              # release build, linked into ~/.local/bin
```

The self-check is `src/ui/debug.rs`; a new key or panel gets checks there.
It never touches the desktop. Dialogs come out blank in its pictures; that
is Broadway, not the app.

## Rules

- The code is formatted by hand in long lines; do not run `cargo fmt` on
  the repo.
- One branch and PR per task; `master` takes fast-forwards only (ruleset:
  linear history, no force-push). Push only when the owner asks.
- The repo is public: no customer data, paths or names in code, tests or
  pictures.

## Decisions

ADRs live in the vault: `~/ObsidianVault/Projects/omamv/ADR/` (index:
`ADR/Index.md`). Read the ones that touch what you change, cite `ADR-nnnn` in
the commit that implements one, and write a new decision there, nowhere else.
