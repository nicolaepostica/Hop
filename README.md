# Hop (Rust)

KVM-over-IP: one keyboard and mouse, many computers. Rust rewrite of the
original C++ [Hop](https://github.com/hop/input-leap).

The legacy C++ tree has been moved under `old/` and remains only for
reference. The Rust rewrite is a clean-slate implementation — see
[`specs/rust-rewrite.md`](specs/rust-rewrite.md) for design and
[`specs/milestones/`](specs/milestones/) for the phased plan.

## Status

Early development. Milestone M0 (workspace skeleton, CI, tooling) is the
only completed phase.

## Build

```
cargo build --workspace
```

MSRV: **1.75** (required for async-fn-in-trait). Latest stable recommended.

## Binaries

| Binary              | Purpose                                              |
|---------------------|------------------------------------------------------|
| `hops`       | Server — shares the primary machine's keyboard/mouse |
| `hopc`       | Client — receives input on a secondary machine       |
| `hop-migrate`| One-shot migration of legacy XML configs to TOML     |

## Development

Run the full local CI pipeline:

```
cargo xtask ci
```

Individual steps:

```
cargo xtask fmt    # cargo fmt --all
cargo xtask lint   # cargo clippy -D warnings
cargo xtask test   # cargo nextest (or cargo test fallback)
cargo xtask deny   # cargo deny check
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for conventions.

## License

GPL-2.0-only, inherited from upstream Hop.
