# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Hop

Hop is a KVM-over-IP software fork of Barrier/Synergy that lets one keyboard and mouse control multiple computers. A **server** (primary machine) shares its input devices with **clients** (secondary machines) over TCP/IP, optionally with SSL/TLS.

## Build Commands

```bash
# Quick build using the provided script
./clean_build.sh

# Manual build
cmake -DCMAKE_BUILD_TYPE=Debug -S . -B build -GNinja
cmake --build build --parallel

# Release build with install
cmake -DCMAKE_BUILD_TYPE=Release -S . -B build -GNinja
cmake --build build --parallel --target install
```

Key CMake options:

- `-DINPUTLEAP_BUILD_GUI=OFF` — skip the Qt GUI
- `-DINPUTLEAP_BUILD_TESTS=OFF` — skip tests
- `-DINPUTLEAP_BUILD_LIBEI=ON` — enable libei/Wayland support (experimental)
- `-DQT_DEFAULT_MAJOR_VERSION=6` — use Qt6 (default is Qt5)
- `-DINPUTLEAP_USE_EXTERNAL_GTEST=ON` — use system-installed Google Test

## Running Tests

```bash
# Run all tests
ctest --test-dir build --verbose

# Run a specific test by name pattern
ctest --test-dir build -R <test_name> --verbose

# Run the test binaries directly
./build/bin/unittests
./build/bin/integtests
./build/bin/guiunittests
```

Test locations:

- `src/test/unittests/` — unit tests (Google Test)
- `src/test/integtests/` — integration tests (network, IPC, platform)
- `src/gui/test/` — GUI unit tests

## Architecture Overview

### Executables

| Binary | Purpose |
|--------|---------|
| `hops` | Server — runs on the primary machine, shares its keyboard/mouse |
| `hopc` | Client — runs on secondary machines, receives input |
| `hop` | GUI — egui desktop app to configure and launch server/client |
| `hopd` | Daemon — Windows service enabling pre-login input sharing |

### Library Structure (`src/lib/`)

- **`base/`** — Event queue, logging, string utilities, foundational types
- **`arch/`** — OS abstraction layer (threading, networking, system calls)
- **`platform/`** — Platform implementations:
  - `MSWindows*` — Windows
  - `OSX*` — macOS (Carbon)
  - `XWindows*` — Linux/X11
  - `Ei*` — libei/Wayland
- **`net/`** — TCP sockets, SSL/TLS (`SecureSocket`, `SecureListenSocket`)
- **`inputleap/`** — Core protocol types, screen management, key/mouse event types
- **`client/`** — Client-side protocol logic (`ServerProxy`, `Client`)
- **`server/`** — Server-side protocol logic (`ClientProxy`, `Server`, `ClientListener`)
- **`ipc/`** — GUI ↔ daemon inter-process communication
- **`mt/`** — Threading primitives

### Data Flow

```block
User Input on Primary
       ↓
Platform Layer (XWindows/OSX/MSWindows)
       ↓
Server (hops)  ←──IPC──→  GUI (hop)
       ↓ TCP/SSL port 24800
Client (hopc)
       ↓
Platform Layer (secondary machine)
       ↓
Injected Input Events
```

### Network Protocol

Custom binary protocol (Synergy-compatible, version 1.6) over TCP port 24800:

- Handshake: `HELLO` / `HELLOBACK`
- Message codes are 4-byte identifiers (e.g., `kMsgCEnter`, `kMsgDKeyDown`)
- Keep-alive every 3 seconds; 3 missed = disconnect
- Optional SSL/TLS with certificate pinning
- Protocol constants and message definitions: `src/lib/inputleap/protocol_types.h`

### Event System

The codebase is event-driven. `EventQueue` dispatches `Event` objects to `EventTarget` instances. Platform layers post events (key presses, mouse moves) into the queue; server/client layers consume them.

## Coding Conventions

- **Namespace:** `inputleap`
- **Classes:** PascalCase; interface classes prefixed with `I` (e.g., `IScreen`, `INode`)
- **Functions:** camelCase
- **Constants:** `kPascalCase` prefix (e.g., `kDefaultPort`, `kProtocolMajorVersion`)
- **Platform implementations:** prefixed with platform name (`MSWindows*`, `OSX*`, `XWindows*`, `Ei*`)
- 4-space indentation, LF line endings, UTF-8

## Platform Notes

- **Windows:** Requires MSVC (VS 2019/2022), 64-bit only
- **macOS:** Uses Carbon framework; Universal binaries (Intel + Apple Silicon)
- **Linux/X11:** Full support; drag & drop not supported
- **Linux/Wayland:** Requires libei (`-DINPUTLEAP_BUILD_LIBEI=ON`), experimental; clipboard sharing not supported
- **FreeBSD/OpenBSD:** X11 only

Feature matrix: clipboard sharing works on Windows, macOS, Linux/X11. Drag & drop works on Windows and macOS only.
