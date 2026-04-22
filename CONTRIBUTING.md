# Contributing

## Local CI

Before pushing, run:

```
cargo xtask ci
```

This runs the same set of checks as GitHub Actions: `rustfmt --check`,
`clippy -D warnings`, `cargo build`, tests via `cargo-nextest` (falls back
to `cargo test`), and `cargo-deny` if installed.

Install optional tools:

```
cargo install cargo-nextest cargo-deny
```

## Coding conventions

- **Edition:** 2021
- **MSRV:** 1.75 — do not use features that require newer toolchains
  without bumping both `rust-version` in `Cargo.toml` and `msrv` in
  `clippy.toml`.
- **Indentation:** 4 spaces, LF line endings, UTF-8 (enforced by
  `.editorconfig` and `rustfmt.toml`).
- **Max line width:** 100 characters.
- **Imports:** grouped std / external / local, sorted; granularity `Module`.
  These are nightly-only `rustfmt` options; on stable they are accepted
  but not applied, so keep imports manually tidy.
- **Errors:** `thiserror` in library crates, `anyhow` in binaries.
- **Logging:** `tracing`; use structured fields (`%peer`, `?reason`).
- **`unsafe`:** forbidden except inside `platform/*/ffi.rs` modules; each
  `unsafe { ... }` block needs a `// SAFETY:` comment stating the
  invariants that make it sound.

## Branch & commit hygiene

- Rebase onto `master` before opening a PR; no merge commits on feature
  branches.
- Commit messages: imperative mood (`fix`, `add`, `remove`), ≤ 72 chars
  for the summary line, optional body after a blank line.

## Spec-first workflow

Non-trivial work is tracked by specs under `specs/` and milestone plans
under `specs/milestones/`. Read the milestone spec before starting on
that milestone's work.
