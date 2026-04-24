<div align="center">
  <img src="assets/hop.png" alt="Hop logo" width="128" />

  <h1>Hop</h1>

  <p><em>One keyboard and mouse. All your computers.</em></p>

  <p>
    <a href="https://github.com/nicolaepostica/Hop/actions"><img src="https://img.shields.io/github/actions/workflow/status/nicolaepostica/Hop/release.yml?branch=main&label=build" alt="build status"/></a>
    <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="license"/></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-1.75%2B-orange" alt="MSRV"/></a>
  </p>
</div>

---

## What is Hop?

Hop is a software KVM: move your cursor off the edge of one screen and
it jumps to the next computer, keyboard and clipboard following along.
One primary machine shares its input devices with any number of
secondaries over the local network — no extra hardware required.

Hop is a clean-slate Rust rewrite of the
[Barrier](https://github.com/debauchee/barrier) /
[Input Leap](https://github.com/input-leap/input-leap) family, focused
on:

- **Simplicity** — one binary per role, one TOML file per concern.
- **Modern crypto** — mutual TLS with trust-on-first-use fingerprint
  pinning; no PKI to manage.
- **A UI that doesn't lie** — Hop's desktop app (`hop`) shows you the
  fingerprint on day one, no hunting through dialogs.

## Status

Early development. The transport stack, screen-crossing coordinator,
clipboard, and file transfers all work end-to-end; the desktop UI is
functional but still a thin shell around the CLI daemons (see
[Roadmap](#roadmap)).

| Milestone | Scope                                         | State |
|-----------|-----------------------------------------------|-------|
| M0        | Workspace skeleton, CI, tooling               |  ✅   |
| M1        | Wire protocol (CBOR) + codec                  |  ✅   |
| M2        | TCP, mTLS, handshake, keep-alive              |  ✅   |
| M9        | File-clipboard transfer engine                |  ✅   |
| M10       | IPC socket for GUI ↔ daemon                   |  ✅   |
| M11       | Screen-crossing coordinator (server-side)     |  ✅   |
| M12       | Release packaging (`.app`, `.deb`, `.msi`)    |  ✅   |

Platform backends: **Linux/X11** ✅, **macOS** ✅, **Windows** ✅,
**Linux/Wayland** (via libei) experimental.

## Install

### Pre-built binaries

Every `v*` tag publishes signed artefacts to
[Releases](https://github.com/nicolaepostica/Hop/releases):

| OS                  | Download             | Installs to                         |
|---------------------|----------------------|-------------------------------------|
| macOS (Apple Silicon) | `Hop-aarch64.dmg`  | drag into `/Applications`           |
| macOS (Intel)         | `Hop-x86_64.dmg`   | drag into `/Applications`           |
| Debian / Ubuntu     | `hop_X.Y.Z_amd64.deb` | `sudo dpkg -i …`                |
| Windows 10/11       | `Hop-X.Y.Z.msi`      | double-click to install             |
| Any Linux           | `hop-x86_64-linux.tar.gz` | extract, run `./hop`          |

After install you get three commands: `hop` (desktop UI), `hops`
(server daemon), `hopc` (client daemon).

### From source

Requires Rust **1.75+** and standard GUI build deps on Linux
(`libgtk-3-dev libxkbcommon-dev libx11-dev libwayland-dev`).

```bash
git clone https://github.com/nicolaepostica/Hop.git
cd Hop
cargo build --release --workspace
# produced at:  target/release/{hop, hops, hopc}
```

## Quick start

### Using the desktop app

```bash
hop
```

On each machine:

1. Pick **Server** (the one sharing the keyboard/mouse) or **Client**
   (the one borrowing them).
2. Copy the **fingerprint** displayed in the UI and paste it into the
   other machine's trust settings.
3. Hit **Start** / **Connect** — done.

### Using the CLI

On the primary (server) machine:

```bash
# First run generates ~/.local/share/hop/tls/{cert,key}.pem
hops fingerprint show         # copy this sha256:… string
hops fingerprint add laptop sha256:<client-fingerprint>
hops                          # listen on 0.0.0.0:25900
```

On the secondary (client) machine:

```bash
hopc fingerprint show         # copy your fingerprint to the server
hopc fingerprint add desk sha256:<server-fingerprint>
hopc --server 192.168.1.10:25900 --name laptop
```

Now move the cursor off the edge of the server's screen — it jumps.

## How it works

```text
 ┌──────────────────────┐        ┌──────────────────────┐
 │  desk (server)       │        │  laptop (client)     │
 │                      │        │                      │
 │  keyboard ─┐         │  mTLS  │                      │
 │  mouse   ──┤         │◄──────►│                      │
 │            ▼         │ :25900 │                      │
 │   Coordinator ──────── msg ───► Coordinator proxy    │
 │   (layout + routing) │        │   ▼                  │
 │                      │        │  inject input        │
 └──────────────────────┘        └──────────────────────┘
```

- **Transport:** TCP + TLS 1.3 (rustls). Both sides present
  self-signed certificates; verification is anchored to a local
  fingerprint DB, not a CA chain. This is the same "SSH-style" trust
  model SSH itself uses.
- **Coordinator:** a pure state-machine on the server owns the virtual
  screen layout. When the cursor crosses a rect boundary it releases
  held keys/buttons on the old screen, emits `ScreenLeave`, bumps a
  monotonic sequence number, and sends `ScreenEnter` to the new peer.
- **Screen layout** lives in `~/.config/hop/layout.toml`:

  ```toml
  primary = "desk"

  [[screen]]
  name = "desk"
  origin_x = 0
  origin_y = 0
  width = 1920
  height = 1080

  [[screen]]
  name = "laptop"
  origin_x = -1440   # to the left of desk
  origin_y = 90
  width = 1440
  height = 900
  ```

  Edit and restart; a live-reload IPC path is on the roadmap.

- **Clipboard** sync works in both directions on X11, macOS, Windows.
  Files can be copied too (M9): copy a file on one side, paste on the
  other — it streams via the same TLS socket and lands in your
  Downloads folder.

See [`specs/architecture.md`](specs/architecture.md) for the full
design and [`specs/milestones/`](specs/milestones/) for phased plans.

## Configuration

| File                                       | Purpose                                            |
|--------------------------------------------|----------------------------------------------------|
| `~/.config/hop/config.toml`                | listen address, TLS dir, display name              |
| `~/.config/hop/layout.toml`                | virtual screen arrangement                         |
| `~/.local/share/hop/tls/{cert,key}.pem`    | TLS identity (generated on first run)              |
| `~/.local/share/hop/fingerprints.toml`     | trusted peer fingerprints                          |

Locations follow the [XDG base directory spec](https://specifications.freedesktop.org/basedir-spec/)
on Linux and the `directories` crate's conventions on macOS and
Windows.

CLI flags override file/env values; `hops --help` shows the full list.

## Development

```bash
cargo xtask ci          # fmt + clippy + test + deny, all-in-one
cargo xtask fmt         # cargo fmt --all
cargo xtask lint        # cargo clippy -- -D warnings
cargo xtask test        # cargo nextest (or cargo test fallback)
cargo xtask deny        # cargo deny check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for coding conventions, the
spec-first workflow, and branch hygiene.

### Project structure

```text
crates/
├── common/       shared types (IDs, modifier masks, clipboard formats)
├── protocol/     wire messages + CBOR codec
├── net/          TCP + TLS + fingerprint DB + handshake
├── ipc/          local socket for GUI ↔ daemon
├── config/       layered TOML + env + CLI settings loader
├── server/       accept loop, coordinator, ClientProxy
├── client/       connect + input injection
├── platform/     PlatformScreen trait + x11/macos/windows/ei backends
├── transfer/     file-clipboard engine (M9)
└── hop-ui/       egui desktop UI library

bins/
├── hops          server daemon binary
├── hopc          client daemon binary
├── hop           desktop UI binary (egui)
└── hop-migrate   one-shot XML → TOML migration for legacy configs

assets/           brand files (hop.svg is the source; .png/.icns/.ico derived)
scripts/          gen-icons.sh + CI helpers
specs/            design docs + per-milestone plans
```

## Roadmap

- **UI ↔ daemon wiring.** Start/Stop in the GUI today is still a
  placeholder toggle; in-progress work spawns `hops`/`hopc` runtimes
  as children of the `hop` process.
- **Visual layout editor.** Drag-and-drop screen boxes; writes
  `layout.toml` atomically.
- **Live layout reload.** IPC method so the server picks up layout
  edits without a restart.
- **System tray + first-run wizard.**
- **Wayland parity.** Make the libei backend non-experimental.
- **Windows service mode** (`hopd`).

## Acknowledgements

Hop stands on the shoulders of:

- [**Synergy**](https://symless.com/synergy) — Chris Schoeneman's
  original 2001 project that defined the genre.
- [**Barrier**](https://github.com/debauchee/barrier) — the
  community fork that kept it alive and open.
- [**Input Leap**](https://github.com/input-leap/input-leap) — the
  active continuation and our C++ reference (preserved under `old/`
  in this repository).

The wire protocol is compatible in spirit with these projects but
intentionally not bit-compatible; Hop uses CBOR-tagged messages and
mTLS instead of Synergy's `kMsg*` framing.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual-licensed as above, without any additional terms
or conditions.
