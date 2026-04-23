# M0 — Скелет воркспейса, CI, tooling

## Цель

Подготовить пустой, но полностью настроенный Cargo workspace с CI, линтами и dev-инструментами. После M0 любой новый код попадает в уже правильно настроенное окружение — не придётся возвращаться и переделывать CI/tooling позже.

## Предпосылки

Нет.

## Scope

**In scope:**
- Cargo workspace с пустыми крейтами из структуры основного спека
- Закреплённые версии всех основных зависимостей в `[workspace.dependencies]`
- CI на GitHub Actions: Linux + macOS + Windows
- Линты, форматирование, deny checks
- `xtask` для dev-команд
- Pinned MSRV

**Out of scope:**
- Любой продуктовый код в крейтах (только `//! TODO`-модули)
- Реальные тесты (только infrastructure check)
- Qt GUI изменения

## Задачи

### Workspace

- [ ] Создать корневой `Cargo.toml` с `[workspace]` + `resolver = "2"`
- [ ] `rust-toolchain.toml` с `channel = "stable"`, зафиксировать MSRV (>= 1.75 для AFIT)
- [ ] Создать пустые крейты-библиотеки:
  - `crates/common/`
  - `crates/protocol/`
  - `crates/net/`
  - `crates/ipc/`
  - `crates/config/`
  - `crates/server/`
  - `crates/client/`
  - `crates/platform/core/`
  - `crates/platform/x11/` (с `#[cfg(target_os = "linux")]` gate на крейт-уровне)
  - `crates/platform/macos/` (с `#[cfg(target_os = "macos")]`)
  - `crates/platform/windows/` (с `#[cfg(windows)]`)
  - `crates/platform/ei/` (с `#[cfg(target_os = "linux")]`)
- [ ] Создать пустые бинарные крейты:
  - `bins/hops/` с `fn main()` печатающим version
  - `bins/hopc/` с `fn main()` печатающим version
  - `bins/hop-migrate/` (за feature flag, не билдится по умолчанию)
- [ ] Создать `xtask/` с заглушками `cargo xtask ci`, `cargo xtask fmt`
- [ ] Каждый крейт — `lib.rs` с `#![deny(warnings, unsafe_code)]` (snap-level, `unsafe` разрешается только в `platform/*/ffi.rs` через `#[allow(unsafe_code)]` локально)

### Dependencies (заполнить `[workspace.dependencies]`)

Предварительный список — конкретные версии подбираются latest stable на момент M0:

- `tokio` (multi-thread, full features disabled by default, per-crate opt-in)
- `tokio-util` (codec)
- `tokio-rustls`, `rustls`, `rustls-pemfile`, `rcgen`
- `bytes`
- `serde`, `serde_json`
- `ciborium`
- `thiserror`, `anyhow`
- `tracing`, `tracing-subscriber`
- `clap` (derive)
- `figment` (toml + env)
- `directories`
- `interprocess`
- `backoff`
- `arc-swap`
- `x11rb` (только в `platform/x11`)
- `reis` (только в `platform/ei`)
- `windows` (только в `platform/windows`)
- `objc2`, `core-graphics` (только в `platform/macos`)
- Dev: `proptest`, `insta`, `tokio-test`, `rstest`

### Конфиги tooling

- [ ] `rustfmt.toml`:
  ```toml
  edition = "2021"
  max_width = 100
  imports_granularity = "Module"
  group_imports = "StdExternalCrate"
  ```
- [ ] `clippy.toml`:
  ```toml
  msrv = "1.75.0"
  avoid-breaking-exported-api = false
  ```
- [ ] `deny.toml` для `cargo-deny`:
  - advisories: deny vulnerabilities
  - licenses: allow MIT/Apache-2.0/BSD-3-Clause/ISC/Unicode-DFS-2016; deny GPL
  - bans: deny multiple versions of `syn`, `tokio`, `rustls`
- [ ] `.gitignore`: `/target`, `.DS_Store`, `*.swp`
- [ ] `.editorconfig` (4 пробела, LF, UTF-8 per CLAUDE.md coding conventions)

### CI (GitHub Actions)

- [ ] `.github/workflows/ci.yml`:
  - Matrix: `{ ubuntu-latest, macos-latest, windows-latest }` × stable toolchain
  - Steps:
    - `cargo fmt --all --check`
    - `cargo clippy --workspace --all-targets -- -D warnings`
    - `cargo build --workspace --all-targets`
    - `cargo nextest run --workspace`
    - `cargo-deny check` (только Linux)
  - Кеширование через `Swatinem/rust-cache@v2`
- [ ] `.github/workflows/release.yml` — заглушка с ручным триггером (наполним в M10)

### xtask

- [ ] `cargo xtask ci` — локально прогоняет тот же набор, что CI
- [ ] `cargo xtask fmt` — `cargo fmt` + форматирование TOML (`taplo fmt`)
- [ ] `cargo xtask udeps` — `cargo +nightly udeps` для выявления неиспользуемых зависимостей (опциональная команда)

### Документация

- [ ] `README.md` в корне:
  - Короткое описание проекта
  - Build instructions (`cargo build --workspace`)
  - Ссылка на `specs/`
- [ ] `CONTRIBUTING.md`:
  - Как запустить CI локально (`cargo xtask ci`)
  - Coding conventions (`snake_case` по Rust, 100-char lines)
  - Где писать тесты

## Acceptance criteria

- [ ] `cargo build --workspace` проходит на Linux/macOS/Windows
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings
- [ ] `cargo fmt --all --check` — clean
- [ ] `cargo nextest run --workspace` — 0 tests, 0 failures (OK для M0)
- [ ] `cargo deny check` — green
- [ ] CI workflow триггерится на push/PR и проходит на всех трёх ОС
- [ ] `./target/release/hops --version` и `./target/release/hopc --version` печатают корректную версию из `Cargo.toml`
- [ ] Все крейты видны в `cargo tree --workspace`

## Тесты

В M0 реального кода нет, но инфраструктура должна быть готова:
- [ ] Dummy `#[test] fn smoke() { assert_eq!(2 + 2, 4); }` в `crates/common/tests/` — чтобы убедиться, что `cargo nextest` действительно что-то прогоняет
- [ ] CI job, падающий при `cargo clippy` с искусственным warning — ручная проверка одной итерацией, потом убрать

## Риски / open questions

1. **Tokio feature-flags в workspace deps:** указать `default-features = false` в workspace root, а конкретные feature-ы включать per-crate? Или наоборот — `full` в workspace, per-crate отключать? Рекомендую первый вариант (явные features → меньше compile time).
2. **`reis` версия нестабильна.** Перед M0 проверить, что актуальный релиз собирается. Если нет — отложить до M6, добавить `reis` в `[workspace.dependencies]` как `optional = true` без вытягивания.
3. **`cargo-deny` на Windows:** исторически флакает на лицензиях — запускать только на Linux, это нормально.
4. **MSRV 1.75:** фиксированный минимум для AFIT-в-трейтах. Если к моменту M0 выйдет новая stable — подтянуть (не наоборот, не опускать ниже 1.75).
5. **Naming:** workspace-package = `hop`? Или оставить `input-leap` и переименовать после удаления C++? Рекомендую `hop` до M10, чтобы не конфликтовать с C++-артефактами в CI на переходном этапе (даже если нет wire-compat, build-артефакты могут пересекаться).
