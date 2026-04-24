# M12 — Release Packaging

## Goal

Land an automated release pipeline: on `git tag v*`, CI produces ready-to-install artefacts for Linux, macOS and Windows. The user downloads the file for their OS, double-clicks — the app starts; for Unix there's also a `curl | sh` installer.

Covers the three pending session-todo items:
- **#7** — generate `hop.icns` from `assets/hop.png`.
- **#8** — macOS code signing + notarization.
- **#9** — GitHub Actions release workflow with a Linux/macOS/Windows matrix.

## Scope

**In scope:**
- Multi-size icons on every platform: `.icns` (macOS), `.ico` (Windows), `.png` + `.desktop` (Linux). Source: `assets/hop.svg`.
- `[package.metadata.bundle]` already lives in `bins/hop/Cargo.toml` (landed in the M11 post-commit).
- GitHub Actions release workflow, triggered on the `v*` git tag.
- macOS: `cargo bundle --format osx` → `.app` → `codesign` → `notarytool` submit → `stapler staple` → pack into `.dmg`.
- Linux: `cargo bundle --format deb` → `.deb`; plus a self-contained `tar.gz` with the binary and `hop.desktop`.
- Windows: `cargo bundle --format msi` or `cargo-wix` → `.msi`.
- SHA256 checksums for every artefact.
- Universal `curl | sh` installer script for Unix (optional).

**Out of scope:**
- Homebrew formula / apt repository — a separate M12.1.
- Windows MSIX / Microsoft Store.
- Flatpak / Snap — a separate M12.2.
- Auto-update mechanism — M13.
- Windows code signing — discussed in §5 but deferred (requires buying a cert or applying to SignPath OSS).

## Architecture

### Directories and files

```
assets/                          # brand source of truth
├── hop.svg                      # master icon (vector)
├── hop.png                      # 512×512 raster, generated from SVG
├── hop.desktop                  # Linux XDG desktop entry
├── hop.icns                     # NEW — macOS icon bundle (#7)
├── hop.ico                      # NEW — Windows multi-size ICO
└── iconset/                     # NEW — intermediate PNGs for .icns
    ├── icon_16x16.png
    ├── icon_16x16@2x.png
    ├── ...
    └── icon_512x512@2x.png

scripts/
└── gen-icons.sh                 # NEW — regenerates .icns/.ico/iconset from hop.svg

.github/
└── workflows/
    └── release.yml              # NEW — M12 main deliverable
```

### What the icons tie into

- `crates/hop-ui/src/lib.rs` — `include_bytes!("../../../assets/hop.png")` (runtime window icon). Unchanged.
- `bins/hop/Cargo.toml` — `[package.metadata.bundle]` already references `../../assets/hop.icns` and `../../assets/hop.png`. After M12 #7 those files exist on disk.

## Implementation details

### 1. Icons (task #7)

**Source:** `assets/hop.svg`. Everything else is derived; regeneration is automated in `scripts/gen-icons.sh`.

**Iconset for macOS** (Apple requires specific filenames):
```
icon_16x16.png       16×16
icon_16x16@2x.png    32×32
icon_32x32.png       32×32
icon_32x32@2x.png    64×64
icon_128x128.png     128×128
icon_128x128@2x.png  256×256
icon_256x256.png     256×256
icon_256x256@2x.png  512×512
icon_512x512.png     512×512
icon_512x512@2x.png  1024×1024
```

**`hop.icns`** is built from the iconset:
- On macOS: `iconutil -c icns assets/iconset -o assets/hop.icns`.
- On Linux / in CI: `png2icns assets/hop.icns assets/iconset/*.png` (from the `icnsutils` package).

**`hop.ico`** for Windows — multi-size ICO with pages 16/32/48/256:
```bash
convert assets/iconset/icon_16x16.png \
        assets/iconset/icon_32x32.png \
        assets/iconset/icon_128x128.png \
        assets/iconset/icon_256x256.png \
        assets/hop.ico
```
(ImageMagick 6 or 7; `magick convert` on 7.x.)

**`scripts/gen-icons.sh`:**
```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Rasterise hop.svg → iconset PNGs
mkdir -p assets/iconset
for s in 16 32 128 256 512 1024; do
  rsvg-convert -w $s -h $s assets/hop.svg -o "assets/iconset/_$s.png"
done
# Rename to Apple convention
mv assets/iconset/_16.png   assets/iconset/icon_16x16.png
mv assets/iconset/_32.png   assets/iconset/icon_16x16@2x.png
cp assets/iconset/icon_16x16@2x.png assets/iconset/icon_32x32.png
mv assets/iconset/_64.png   assets/iconset/icon_32x32@2x.png
# ... (same shape for 128/256/512/1024)

# .icns
if command -v iconutil >/dev/null; then
  iconutil -c icns assets/iconset -o assets/hop.icns
else
  png2icns assets/hop.icns assets/iconset/icon_*.png
fi

# .ico
convert assets/iconset/icon_16x16.png \
        assets/iconset/icon_32x32.png \
        assets/iconset/icon_128x128.png \
        assets/iconset/icon_256x256.png \
        assets/hop.ico
```

**We commit the output:** every generated file (`hop.icns`, `hop.ico`, `iconset/`) goes into the repository. This removes the CI dependency on `rsvg-convert` / `iconutil` and makes builds deterministic. When `hop.svg` changes, a developer runs `./scripts/gen-icons.sh` once and commits.

### 2. macOS signing + notarization (task #8)

**Apple requirements:**
- Apple Developer Program membership — **$99/year**. That provides a Developer ID.
- "Developer ID Application" certificate via Keychain Access → Request Certificate from CA → upload CSR to developer.apple.com → download the `.cer`.
- Export into `.p12` with a password to transport into CI.
- An app-specific password for `notarytool`: appleid.apple.com → Sign-In and Security → App-Specific Passwords.

**GitHub Secrets:**
| Name | What |
|---|---|
| `APPLE_CERT_P12_BASE64` | `base64 < DevID.p12` — the whole cert |
| `APPLE_CERT_PASSWORD` | password to the `.p12` |
| `APPLE_SIGNING_IDENTITY` | string like `"Developer ID Application: Jane Doe (TEAMID12)"` |
| `APPLE_ID` | your Apple ID email |
| `APPLE_APP_PASSWORD` | the app-specific password |
| `APPLE_TEAM_ID` | 10-character team ID |

**CI steps on a macOS runner:**
```yaml
- name: Import Developer ID certificate
  run: |
    echo "${{ secrets.APPLE_CERT_P12_BASE64 }}" | base64 --decode > cert.p12
    security create-keychain -p hop-ci build.keychain
    security default-keychain -s build.keychain
    security unlock-keychain -p hop-ci build.keychain
    security import cert.p12 -k build.keychain \
      -P "${{ secrets.APPLE_CERT_PASSWORD }}" -T /usr/bin/codesign
    security set-key-partition-list -S apple-tool:,apple: \
      -s -k hop-ci build.keychain

- name: Build .app
  run: cd bins/hop && cargo bundle --release --format osx

- name: Codesign
  run: |
    codesign --deep --force --verify --options runtime --timestamp \
      --sign "${{ secrets.APPLE_SIGNING_IDENTITY }}" \
      bins/hop/target/release/bundle/osx/Hop.app

- name: Notarize
  run: |
    ditto -c -k --keepParent \
      bins/hop/target/release/bundle/osx/Hop.app \
      Hop.zip
    xcrun notarytool submit Hop.zip \
      --apple-id "${{ secrets.APPLE_ID }}" \
      --password "${{ secrets.APPLE_APP_PASSWORD }}" \
      --team-id "${{ secrets.APPLE_TEAM_ID }}" \
      --wait
    xcrun stapler staple \
      bins/hop/target/release/bundle/osx/Hop.app

- name: Package as .dmg
  run: |
    hdiutil create -volname "Hop" -srcfolder \
      bins/hop/target/release/bundle/osx/Hop.app \
      -ov -format UDZO Hop.dmg
```

**Without signing:** you can still build and publish; on first launch macOS shows a Gatekeeper warning. The user right-clicks → Open → Open, or runs `xattr -cr /Applications/Hop.app`. The README should cover both paths: signed releases (the recommended flow) and an unsigned instruction for self-builds.

### 3. GitHub Actions release workflow (task #9)

File: `.github/workflows/release.yml`

**Trigger:** `push` tag `v*` + `workflow_dispatch` for manual runs.

**Jobs layout:**

```yaml
on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            bundle: deb
          - os: macos-13        # x86_64 runner
            target: x86_64-apple-darwin
            bundle: osx
          - os: macos-latest    # arm64 runner
            target: aarch64-apple-darwin
            bundle: osx
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            bundle: msi
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2

      # Linux-only GUI deps (egui needs these headers at build time).
      - if: matrix.os == 'ubuntu-latest'
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev \
            libx11-dev libwayland-dev libxkbcommon-dev

      - name: Install cargo-bundle
        run: cargo install cargo-bundle

      - name: Build binary
        run: cargo build --release --bin hop --target ${{ matrix.target }}

      - name: Bundle
        working-directory: bins/hop
        run: cargo bundle --release --format ${{ matrix.bundle }} \
                          --target ${{ matrix.target }}

      # macOS signing lives here (see §2).
      - if: matrix.bundle == 'osx'
        run: ./scripts/ci/macos-sign-and-notarize.sh
        env:
          APPLE_CERT_P12_BASE64: ${{ secrets.APPLE_CERT_P12_BASE64 }}
          APPLE_CERT_PASSWORD: ${{ secrets.APPLE_CERT_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_APP_PASSWORD: ${{ secrets.APPLE_APP_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}

      - name: Compute SHA256
        shell: bash
        run: |
          cd target/${{ matrix.target }}/release/bundle/${{ matrix.bundle }}
          for f in *; do shasum -a 256 "$f" > "$f.sha256"; done

      - uses: actions/upload-artifact@v4
        with:
          name: hop-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/bundle/${{ matrix.bundle }}/*

  release:
    needs: build
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/')
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
      - name: Publish GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          files: dist/**/*
          generate_release_notes: true
```

**Key decisions:**
1. **fail-fast: false** — one OS failing doesn't stop the others; the releaser can rerun a broken job.
2. **Swatinem/rust-cache** is mandatory — without it every build pulls the whole dep tree (>5 min on a runner).
3. **arm64 macOS as a separate matrix row** — a Universal binary is nicer as `lipo`-merged artefact, but for MVP two separate `.app`s (x86_64 and aarch64) are enough. The README explains which to download.
4. **Signing only on macOS** — Windows stays unsigned for now (§5); Linux `.deb` isn't signed (dpkg-sig is optional).
5. **Tag trigger:** the release job publishes only on `v*`. On other pushes (feature branches) the build matrix works as a CI smoke; artefacts stay in actions/artifacts for 7 days.

### 4. Linux installer alternatives

`.deb` is the primary format. Useful additions:
- **AppImage** (`cargo install cargo-appimage && cargo appimage --bin hop`) — portable single file, no root required. Ship it as a second artefact.
- **`.tar.gz`** — just the binary + `assets/hop.desktop` + README, for people who don't want `dpkg -i`.

Recommended install path on Ubuntu/Debian — `.deb`; Arch/Fedora/others — AppImage.

### 5. Windows signing (deferred)

Without signing, the `.msi` works, but SmartScreen shows an "Unknown publisher" warning and the user has to click "More info → Run anyway". Not a blocker — just not premium UX.

Options, when we're ready to sign:
- **SignPath.io** — free for OSS (after approval, ~a week).
- **Sectigo / DigiCert EV cert** — ~$150–400/year; better SmartScreen reputation.
- **Azure Trusted Signing** — ~$10/month, but only for registered publishers.

Deferred until the first public release; in M12 we ship notarized macOS + unsigned Windows.

## Implementation order

1. **`scripts/gen-icons.sh`** + generated `hop.icns`, `hop.ico`, `assets/iconset/` — committed. (Task #7)
   - Check: `cargo bundle --release --format osx` locally on a Mac produces a `Hop.app` that shows our icon in Finder.
2. **`.github/workflows/release.yml`** without signing: Linux + Mac (unsigned) + Windows (unsigned). Tested via `workflow_dispatch` on a feature branch. (Task #9, part 1)
   - Check: a manual workflow run produces 4 artefacts (deb, osx x64, osx arm64, msi), each with a `.sha256`.
3. **Apple Developer ID setup** — manual: registration, CSR, export to `.p12`, upload secrets to GitHub. (Task #8, preparation)
4. **`scripts/ci/macos-sign-and-notarize.sh`** + the matching workflow step. (Task #8, implementation)
   - Check: a staged release off `v0.0.0-rc1`; download the `.dmg`, install, confirm Gatekeeper is quiet.
5. **Full tagged release** — push `v0.1.0`, artefacts land on GitHub Releases automatically. (Milestone exit criterion)

## Test plan

| What | How |
|---|---|
| Icon generation | Manual `./scripts/gen-icons.sh` run + size check via `file assets/hop.icns`, `identify assets/hop.ico`. |
| Linux `.deb` | `sudo dpkg -i hop_*.deb && which hop && hop --version` in a Docker Ubuntu 22.04 container. |
| Linux AppImage | `./Hop-*.AppImage` launch in a clean container without system GTK. |
| macOS `.app` unsigned | `open Hop.app` → expected Gatekeeper dialog → right-click Open. |
| macOS `.app` signed+notarized | `spctl -a -t exec -vv Hop.app` → `source=Notarized Developer ID`. |
| Windows `.msi` | Install on a Windows 11 VM, verify shortcut in Start menu. |
| SHA256 sums | `shasum -a 256 -c hop_*.sha256`. |
| Release workflow | `gh workflow run release.yml --ref <test-branch>` + verify artefacts. |
| Tag-triggered release | `git tag v0.0.0-rc1 && git push --tags` → GitHub Release shows up automatically. |

## Estimate + risks

**Time:**
- #7 icons: **3–4 hours** (including a sanity check on a Mac).
- #9 workflow (unsigned): **1 day** (many iterations via `act` / `workflow_dispatch`, rarely works first try).
- #8 signing: **1–2 hours** of setup + **half a day** of `notarytool` debugging (it usually complains about minor entitlement / hardened-runtime issues the first time).

**Total:** ~3 days of focused work, stretched to a week because of Apple's asynchronous steps (cert issuance, notarization latency).

**Risks:**
- **Notarization failure.** Apple may reject the bundle over missing hardened runtime or bad entitlements. Mitigation: run `notarytool` locally in dry-run first; `--options runtime` is already on the codesign command.
- **arm64 macOS runners.** `macos-latest` on GitHub is arm64 today; x86_64 builds need a `macos-13` runner. Handled in the matrix.
- **cargo-bundle maturity.** Buggy in spots (e.g. `--target aarch64-apple-darwin` sometimes fails). Plan B: manually copy the binary into `.app/Contents/MacOS/` via a script if cargo-bundle breaks.
- **.deb runtime deps.** Non-trivial to guess the minimum set; the current `[package.metadata.bundle]` has a reasonable start, but real testing in a clean Ubuntu container may require adjustments.

## Resolved / deferred decisions

1. **cargo-bundle vs cargo-dist.** cargo-dist is simpler to configure (auto-generates a workflow) but only produces `.tar.gz`/`.zip` — not `.app`/`.deb`/`.msi`. We need native packages, so we pick **cargo-bundle** by hand. If Apple/Microsoft integration later gets harder, we can migrate to cargo-dist for archives + a separate action for bundles.

2. **Iconset under version control.** We considered generating `.icns`/`.ico` on the fly in every CI run. Rejected — it lengthens builds, makes rsvg-convert/iconutil CI-wide dependencies, and gives non-deterministic outputs. Commit the artefacts; regenerate via `scripts/gen-icons.sh`.

3. **Universal macOS binary.** Two separate `.app`s (x86_64 and arm64) — MVP. A Universal (lipo-merged) build is a later optimisation once the pipeline is stable. Users see two clearly-named artefacts on the releases page (`Hop-x86_64.dmg`, `Hop-arm64.dmg`).

4. **Windows signing.** Deferred until the first public release; for M12 we ship unsigned with an honest warning in the README.
