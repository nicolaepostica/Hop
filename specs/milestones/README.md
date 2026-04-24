# Milestones — Rust Rewrite

The Hop Rust rewrite is broken into vertical slices. Each milestone ends with a runnable artefact — a binary you can execute, an integration test that passes, or a working end-to-end demo.

Main spec: [`../architecture.md`](../architecture.md).

## Overview

| M | Artefact | Sub-spec |
|---|---|---|
| [M0](M0-skeleton.md) | Workspace skeleton, CI, tooling | detailed |
| [M1](M1-protocol.md) | `protocol` crate: CBOR messages v1, property tests, golden snapshots | detailed |
| [M2](M2-net-handshake.md) | `net` crate: TCP + TLS + handshake + mock screen + integration test | detailed |
| M3 | `platform/x11`: working server+client between two Linux/X11 machines | written when approached |
| M4 | Clipboard (text/HTML) + `config` crate (TOML) | written when approached |
| M5 | `ipc` crate + GUI adaptation to the new IPC | written when approached |
| M6 | `platform/ei`: Wayland/libei via `reis` + portal | written when approached |
| M7 | `platform/macos` | written when approached |
| M8 | `platform/windows` | written when approached |
| M9 | File clipboard (see [`../architecture.md#file-clipboard-m9`](../architecture.md#file-clipboard-m9)) | written when approached |
| M10 | Windows service mode (`hops --service`) | written when approached |

## Decomposition principles

1. **Vertical slices.** Every milestone ships a runnable artefact, not a "half-finished layer". Keeps CI green and lets us validate hypotheses on live code continuously.
2. **Testability is part of acceptance.** No milestone is "done" until a proptest / integration test covers its contract.
3. **Platform backends are independent milestones.** X11, macOS, Windows, and libei are isolated. A failure in one platform does not block the others.
4. **GUI lands late (M5).** Until then we use the CLI/IPC client for smoke tests.
5. **Cherry-picking allowed.** Milestones are numbered, but if M7 (macOS) becomes more important than M6 — feel free to reorder, as long as the dependencies (M0–M2) are complete.

## Milestone dependencies

```
M0 ─► M1 ─► M2 ─┬─► M3 (x11) ─┬─► M4 (clipboard + config)
                │              │         │
                ├─► M6 (ei)    │         └─► M5 (ipc + GUI)
                ├─► M7 (macos) │                 │
                └─► M8 (win)   │                 └─► M9 (file clipboard)
                               │
                               └─► M10 (win service)  [depends on M8]
```

## Sub-spec format

Every milestone spec contains:
- **Goal** — why this milestone exists
- **Prerequisites** — which milestones must be finished first
- **Scope** — what's in / what's out
- **Tasks** — decomposition into concrete checklist items
- **Acceptance criteria** — how we know it's done
- **Tests** — which tests must appear
- **Risks / open questions** — what could go wrong
