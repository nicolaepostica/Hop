# M12 — Release Packaging

## Цель

Собрать автоматический релизный конвейер: на `git tag v*` из CI выходят
готовые артефакты под Linux, macOS и Windows. Пользователь скачивает
файл под свою OS, двойной клик → приложение запускается; для Unix есть
ещё `curl | sh` установщик.

Покрывает три pending-таска из session-todo:
- **#7** — генерация `hop.icns` из `assets/hop.png`.
- **#8** — macOS code signing + notarization.
- **#9** — GitHub Actions release workflow с matrix Linux/macOS/Windows.

## Scope

**В области:**
- Мультиразмерные иконки на всех платформах: `.icns` (macOS), `.ico`
  (Windows), `.png` + `.desktop` (Linux). Источник — `assets/hop.svg`.
- `[package.metadata.bundle]` уже лежит в `bins/hop/Cargo.toml`
  (заложено в M11 post-commit).
- GitHub Actions release workflow, триггер — git-тег `v*`.
- macOS: `cargo bundle --format osx` → `.app` → `codesign` → `notarytool`
  submit → `stapler staple` → упаковка в `.dmg`.
- Linux: `cargo bundle --format deb` → `.deb`; плюс self-contained
  tar.gz с бинарём и `hop.desktop`.
- Windows: `cargo bundle --format msi` или `cargo-wix` → `.msi`.
- SHA256 checksums для всех артефактов.
- Universal `curl | sh` installer скрипт для Unix (опционально).

**Out of scope:**
- Homebrew formula / apt repository — отдельный M12.1.
- Windows MSIX / Microsoft Store.
- Flatpak / Snap — отдельный M12.2.
- Auto-update mechanism — M13.
- Windows code signing — обсуждается в §5, но сам setup отложен
  (требует покупки сертификата или заявки в SignPath OSS).

## Архитектура

### Директории и файлы

```
assets/                          # brand source of truth
├── hop.svg                      # мастер-иконка (векторная)
├── hop.png                      # 512×512 raster, сгенерировано из SVG
├── hop.desktop                  # Linux XDG desktop entry
├── hop.icns                     # NEW — macOS icon bundle (#7)
├── hop.ico                      # NEW — Windows multi-size ICO
└── iconset/                     # NEW — промежуточные PNG для .icns
    ├── icon_16x16.png
    ├── icon_16x16@2x.png
    ├── ...
    └── icon_512x512@2x.png

scripts/
└── gen-icons.sh                 # NEW — регенерирует .icns/.ico/iconset из hop.svg

.github/
└── workflows/
    └── release.yml              # NEW — M12 main deliverable
```

### Что цепляется за иконки

- `crates/hop-ui/src/lib.rs` — `include_bytes!("../../../assets/hop.png")`
  (рантайм-иконка окна). Не меняется.
- `bins/hop/Cargo.toml` — `[package.metadata.bundle]` уже ссылается
  на `../../assets/hop.icns` и `../../assets/hop.png`. После M12 #7
  файлы физически появятся.

## Детали реализации

### 1. Иконки (task #7)

**Источник:** `assets/hop.svg`. Всё остальное — производные; их
регенерация автоматизирована в `scripts/gen-icons.sh`.

**Iconset для macOS** (Apple требует конкретные имена файлов):
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

**`hop.icns`** собирается из iconset'а:
- На macOS: `iconutil -c icns assets/iconset -o assets/hop.icns`.
- На Linux/в CI: `png2icns assets/hop.icns assets/iconset/*.png`
  (из пакета `icnsutils`).

**`hop.ico`** для Windows — multi-size ICO со страницами 16/32/48/256:
```bash
convert assets/iconset/icon_16x16.png \
        assets/iconset/icon_32x32.png \
        assets/iconset/icon_128x128.png \
        assets/iconset/icon_256x256.png \
        assets/hop.ico
```
(ImageMagick 6 или 7; `magick convert` на 7.x.)

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
# ... (аналогично для 128/256/512/1024)

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

**Коммитим результат:** все сгенерированные файлы (`hop.icns`, `hop.ico`,
iconset/) идут в репозиторий. Это убирает зависимость CI от
`rsvg-convert` / `iconutil` и делает билды детерминированными. При
изменении `hop.svg` разработчик один раз запускает
`./scripts/gen-icons.sh` и коммитит.

### 2. macOS signing + notarization (task #8)

**Требования Apple:**
- Apple Developer Program membership — **$99/год**. Даёт Developer ID.
- Сертификат "Developer ID Application" из Keychain Access → Request
  Certificate from CA → upload CSR в developer.apple.com → скачать `.cer`.
- Экспорт в `.p12` c паролем для переноса в CI.
- App-specific password для `notarytool`: appleid.apple.com → Sign-In
  and Security → App-Specific Passwords.

**GitHub Secrets:**
| Имя | Что |
|---|---|
| `APPLE_CERT_P12_BASE64` | `base64 < DevID.p12` — весь сертификат |
| `APPLE_CERT_PASSWORD` | пароль от `.p12` |
| `APPLE_SIGNING_IDENTITY` | строка вида `"Developer ID Application: Jane Doe (TEAMID12)"` |
| `APPLE_ID` | твой Apple ID email |
| `APPLE_APP_PASSWORD` | app-specific password |
| `APPLE_TEAM_ID` | 10-символьный team ID |

**CI-шаги на macOS runner:**
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

**Без подписи:** релиз всё равно можно собирать и выкладывать; при
первом запуске macOS покажет Gatekeeper warning. Пользователь должен
будет кликнуть правой → Open → Open, или выполнить
`xattr -cr /Applications/Hop.app`. В README отражаем оба сценария:
подписанные релизы (рекомендуемый путь) и unsigned-инструкция для
self-build.

### 3. GitHub Actions release workflow (task #9)

Файл: `.github/workflows/release.yml`

**Триггер:** `push` tag `v*` + `workflow_dispatch` для ручного запуска.

**Структура jobs:**

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

**Ключевые решения:**
1. **fail-fast: false** — падение одной OS не стопает остальные;
   релизёр может ручно перезапустить сломавшийся job.
2. **Swatinem/rust-cache** обязателен — без него каждый build тянет
   весь dep tree (>5 мин на runner).
3. **arm64 macOS как отдельная matrix-строка** — Universal binary
   лучше собирать отдельными шагами и сшивать `lipo`, но в MVP
   достаточно двух отдельных .app (x86_64 и aarch64). В README
   напишем какой качать.
4. **Подпись только на macOS** — Windows пока unsigned (§5);
   Linux `.deb` не подписываем (секция dpkg-sig опциональна).
5. **Тег-триггер:** release-job упаковывает только при `v*`. На
   других push'ах (feature branches) build matrix работает как CI
   smoke, артефакты остаются в actions/artifacts на 7 дней.

### 4. Linux Installer альтернатив

`.deb` — основной формат. Дополнительно стоит:
- **AppImage** (`cargo install cargo-appimage && cargo appimage --bin hop`)
  — portable single-file, не требует root. Добавим как второй artefact.
- **`.tar.gz`** — просто бинарь + `assets/hop.desktop` + README. Для
  тех кто не хочет `dpkg -i`.

Рекомендуемый путь установки Ubuntu/Debian — `.deb`; Arch/Fedora/
other — AppImage.

### 5. Windows signing (отложено)

Без подписи `.msi` сработает, но SmartScreen покажет "Unknown
publisher" warning и пользователь должен будет нажать "More info →
Run anyway". Это не блокирует — просто не premium UX.

Варианты когда будем подписывать:
- **SignPath.io** — бесплатно для OSS (после approval, неделя).
- **Sectigo / DigiCert EV cert** — ~$150-400/год; лучше репутация
  у SmartScreen.
- **Azure Trusted Signing** — ~$10/мес, но только для зарегистрированных
  публишеров.

Откладывается до момента первого публичного релиза; в M12 ограничиваемся
notarized macOS + unsigned Windows.

## Порядок имплементации

1. **`scripts/gen-icons.sh`** + сгенерированные `hop.icns`, `hop.ico`,
   `assets/iconset/` — закоммичены. (Task #7)
   - Проверка: `cargo bundle --release --format osx` локально на Mac
     даёт `Hop.app` с нашей иконкой в Finder.
2. **`.github/workflows/release.yml`** без подписи: Linux + Mac
   (unsigned) + Windows (unsigned). Тестируется через
   `workflow_dispatch` на feature-branch. (Task #9, часть 1)
   - Проверка: ручной запуск workflow производит 4 артефакта
     (deb, osx x64, osx arm64, msi), у каждого есть `.sha256`.
3. **Apple Developer ID setup** — вручную: регистрация, CSR, экспорт
   в `.p12`, заливка secrets в GitHub. (Task #8, часть preparation)
4. **`scripts/ci/macos-sign-and-notarize.sh`** + соответствующий
   шаг в workflow. (Task #8, часть implementation)
   - Проверка: staged релиз с тега `v0.0.0-rc1`; скачать `.dmg`,
     установить, проверить что Gatekeeper не ругается.
5. **Полноценный тегированный релиз** — пуш тега `v0.1.0`, artifacts
   автоматически лежат в GitHub Releases. (Milestone exit criterion)

## Тестовый план

| Что | Как |
|---|---|
| Icon generation | Ручной ран `./scripts/gen-icons.sh` + проверка размеров через `file assets/hop.icns`, `identify assets/hop.ico`. |
| Linux `.deb` | `sudo dpkg -i hop_*.deb && which hop && hop --version` в Docker Ubuntu 22.04. |
| Linux AppImage | `./Hop-*.AppImage` запуск в чистом контейнере без системного GTK. |
| macOS `.app` unsigned | `open Hop.app` → ожидаемый Gatekeeper dialog → right-click Open. |
| macOS `.app` signed+notarized | `spctl -a -t exec -vv Hop.app` → `source=Notarized Developer ID`. |
| Windows `.msi` | Установить в Windows 11 VM, проверить shortcut в Start menu. |
| SHA256 sums | `shasum -a 256 -c hop_*.sha256`. |
| Release workflow | `gh workflow run release.yml --ref <test-branch>` + проверка артефактов. |
| Tag-triggered release | `git tag v0.0.0-rc1 && git push --tags` → GitHub Release появляется автоматически. |

## Оценка + риски

**Время:**
- #7 icons: **3-4 часа** (включая sanity-проверку на Mac).
- #9 workflow (без подписей): **1 день** (много итераций через
  `act` / `workflow_dispatch`, редко получается с первого раза).
- #8 signing: **1-2 часа** настройки + **½ дня** отладки
  notarytool (первый раз он часто ругается на minor issues в
  entitlements/hardened runtime).

**Итого:** ~3 дня чистой работы, растянуть на неделю из-за Apple
asynchronous steps (выпуск cert, notarization latency).

**Риски:**
- **Notarization fail.** Apple может отклонить bundle из-за
  отсутствия hardened runtime / неправильных entitlements. Митигация:
  локально погонять `notarytool` в dry-run, `--options runtime` уже в
  кодсайн-команде.
- **arm64 macOS runners.** `macos-latest` на GitHub сейчас arm64;
  x86_64 билды требуют `macos-13` runner. Учтено в matrix.
- **cargo-bundle зрелость.** Местами багованный (напр.
  `--target aarch64-apple-darwin` порой проваливается). План B —
  вручную копировать бинарь в `.app/Contents/MacOS/` через скрипт,
  если cargo-bundle ломается.
- **.deb runtime deps.** Нетривиально угадать минимальный набор; в
  текущем `[package.metadata.bundle]` задан разумный старт, но
  реальное тестирование в чистом Ubuntu контейнере может потребовать
  корректировок.

## Resolved / deferred decisions

1. **cargo-bundle vs cargo-dist.** cargo-dist проще настраивается
   (auto-generates workflow), но делает только `.tar.gz`/`.zip` — не
   `.app`/`.deb`/`.msi`. Нам нужны native-пакеты, поэтому выбираем
   **cargo-bundle** вручную. Если в будущем Apple/Microsoft
   integration усложнится — можно мигрировать на cargo-dist для
   archives + отдельный action для bundle.

2. **Iconset под контролем версий.** Рассматривался вариант
   генерировать `.icns`/`.ico` on-the-fly в каждом CI run. Отклонено
   — увеличивает время сборки, вводит rsvg-convert/iconutil как
   CI dep, даёт недетерминированные выходы. Коммитим готовые
   артефакты; регенерация — через `scripts/gen-icons.sh`.

3. **Universal macOS binary.** Два отдельных `.app` (x86_64 и arm64)
   — MVP. Universal (lipo-merged) — отдельная оптимизация когда
   pipeline устаканится. Пользователь на странице релизов видит два
   артефакта с понятными именами (`Hop-x86_64.dmg`, `Hop-arm64.dmg`).

4. **Windows signing.** Отложено до первого публичного релиза;
   в M12 выпускаем unsigned с честным warning в README.
