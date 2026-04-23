# Spec: Hop — Переписывание на Rust

## Цель

Полностью переписать Hop на Rust. Заменить ручной event loop, самописную абстракцию потоков и OpenSSL на современный стек: `tokio`, `rustls`, `tracing`. Отказаться от обратной совместимости с C++-версией и Synergy/Barrier — это даёт возможность спроектировать сетевой протокол, конфигурацию и IPC с нуля, по-Rust-овски, без legacy wire-форматов.

Мотивы: устранение класса memory-safety багов, нативная поддержка Wayland через libei/portal без FFI-костылей, упрощение кода за счёт стандартных крейтов, type-safe протокол с эволюцией через версионирование.

## Background

Текущая реализация — C++14. Использует самописный `EventQueue`, `SocketMultiplexer` с polling-потоком, платформенные `Arch*`-фасады, OpenSSL напрямую, XML-конфиг (наследие Synergy), бинарный wire-протокол с 4-байтовыми ASCII-кодами сообщений и big-endian кодированием.

**Обратная совместимость с C++-версией Hop и с Synergy/Barrier НЕ требуется.** Переходный период между C++ и Rust отсутствует — пользователи обновляют сервер и клиенты одновременно. Это позволяет снять все legacy-ограничения wire-формата, конфигурации и IPC.

## Scope

**In scope:**
- `hops` (сервер) и `hopc` (клиент) на Rust
- Новый сетевой протокол v1 на CBOR (RFC 8949) + length-delimited фрейминге
- TLS через `rustls` + self-signed cert с fingerprint-верификацией
- Платформенные backend-ы: X11 (`x11rb`), macOS (`core-graphics`/`objc2`), Windows (`windows-rs`), Wayland (libei через `reis`)
- Clipboard синхронизация (текст, HTML; файлы — отдельный спек)
- IPC GUI↔daemon через Unix socket / Named pipe (`interprocess` crate)
- Конфигурация в TOML; одноразовый инструмент миграции со старого XML
- Тесты: unit (`proptest` round-trip) + integration (сеть, IPC, платформы)

**Out of scope первой итерации:**
- Qt GUI заменён на новый egui-бинарь `hop`
- Drag & drop — после MVP
- File clipboard — отдельный спек (`specs/file-clipboard.md`), milestone M9
- Windows service (`hopd`) — тонкая обёртка через `windows-service` крейт в M10, не отдельный codebase

## Requirements

1. Бинарники `hops` и `hopc` на Rust замещают C++-версии; пользователь обновляется single-step одновременно на всех машинах (сервер и клиенты).
2. Все I/O — неблокирующие, на `tokio`. Никакого polling-потока, никакого самописного multiplexer'а.
3. TLS через `rustls` (`tokio-rustls`); fingerprint DB в TOML-формате с комментариями.
4. Платформенный слой — трейт `PlatformScreen` с AFIT (`-> impl Future`); `#[async_trait]` только там, где AFIT не работает (dyn-trait).
5. Wayland-backend через libei (`reis` крейт за внутренним трейтом `EiBackend`) + `xdg-desktop-portal` (`zbus`).
6. IPC через `interprocess::local_socket` (Unix socket на Linux/macOS, Named pipe на Windows); JSON-RPC-подобный протокол. Флаг `--ipc-tcp=<port>` для remote-GUI.
7. Конфигурация в TOML через `serde` + `figment` (layered: файл → env → CLI). Пути через `directories` крейт (XDG-compliant на Linux).
8. Логирование — `tracing` + `tracing-subscriber` со structured fields; IPC-прокидывание логов в GUI как отдельный subscriber layer.
9. Ошибки — `thiserror` в крейтах-библиотеках, `anyhow` только в бинарях.
10. CI: `clippy -D warnings`, `rustfmt --check`, `cargo-nextest`, `cargo-deny` (licenses, advisories, duplicates).
11. `unsafe` разрешён только в `platform/*/ffi.rs`-модулях с документированными инвариантами (`// SAFETY: ...` на каждый блок).

## User / system flow

```
[Первичная машина]                          [Вторичная машина]
PlatformScreen (X11/macOS/Win/EI)           PlatformScreen (X11/macOS/Win/EI)
        |                                           |
   InputEvent stream                         inject_*(key/mouse/...)
        |                                           |
    Server task  ──── TCP:24800 / TLS ────  Client task
        |                                           |
   ScreenRouter                              ServerProxy
        |
    IPC ── Unix socket / Named pipe ── GUI (Qt)
```

**Handshake (v1):**
1. Сервер слушает `0.0.0.0:24800`.
2. Клиент подключается, TLS handshake (`tokio-rustls`). Сервер проверяет fingerprint клиента по локальной DB.
3. Обмен сообщениями `Hello` (CBOR), содержащими `protocol_version: u16`, `display_name: String`, `capabilities: Vec<Capability>`.
4. Сервер шлёт `DeviceInfoRequest` → клиент отвечает `DeviceInfo` с размерами экрана.
5. Соединение активно; `KeepAlive` каждые 3 с в обе стороны; 3 пропуска = `Disconnect { reason: KeepAliveTimeout }`.

## Technical approach

### Структура воркспейса (Cargo workspace)

```
hop/
  Cargo.toml                   # [workspace] + [workspace.dependencies]
  rust-toolchain.toml          # pinned MSRV (stable, >= 1.75 для AFIT)
  rustfmt.toml
  clippy.toml
  deny.toml                    # cargo-deny конфиг
  crates/
    common/                    # KeyId, ButtonId, ClipboardId, ModifierMask и пр.
    protocol/                  # CBOR-схема сообщений, codec
    net/                       # TcpListener/Stream, tokio-rustls, framing
    ipc/                       # interprocess + JSON-RPC
    config/                    # TOML через serde + figment
    server/                    # Логика сервера, ScreenRouter
    client/                    # Логика клиента, ServerProxy
    platform/
      core/                    # Трейт PlatformScreen, общие типы
      x11/
      macos/
      windows/
      ei/                      # Wayland/libei
  bins/
    hops/               # сервер
    hopc/               # клиент
    hop-migrate/        # one-shot миграция XML → TOML (необяз. бинарь)
  xtask/                       # dev-команды: xtask ci, xtask release, ...
```

### Ключевые решения

**Async runtime:** `tokio` multi-thread. Платформенные события (X11 / libei fd) читаются в отдельной `tokio::task` через `tokio::io::unix::AsyncFd`; события идут в `mpsc`-канал. Ядро сервера/клиента — `tokio::select!` цикл по каналам и сетевому IO.

**Протокол (`protocol` crate):**
- Фрейминг: `tokio_util::codec::LengthDelimitedCodec`, 4-byte BE length prefix, `max_frame_length = 16 MiB`.
- Сериализация: CBOR через `ciborium` (RFC 8949, кросс-языковая реализуемость).
- Сообщения — `enum Message` с `#[serde(tag = "type")]`:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Hello(HelloPayload),
    DeviceInfoRequest,
    DeviceInfo(DeviceInfoPayload),
    KeyDown      { key: KeyId, mods: ModifierMask },
    KeyUp        { key: KeyId, mods: ModifierMask },
    KeyRepeat    { key: KeyId, mods: ModifierMask, count: u16 },
    MouseMove    { x: i32, y: i32 },
    MouseRelMove { dx: i32, dy: i32 },
    MouseButton  { button: ButtonId, down: bool },
    MouseWheel   { dx: i32, dy: i32 },
    ScreenEnter  { x: i32, y: i32, seq: u32, mask: ModifierMask },
    ScreenLeave,
    ClipboardGrab { id: ClipboardId, seq: u32 },
    ClipboardData { id: ClipboardId, format: ClipboardFormat, data: Bytes },
    KeepAlive,
    Disconnect { reason: DisconnectReason },
    // расширения: file-clipboard сообщения — см. specs/file-clipboard.md
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloPayload {
    pub protocol_version: u16,
    pub display_name: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Capability {
    FileClipboard,
    UnicodeClipboard,
    ClipboardHtml,
    // future-proof: serde игнорирует неизвестные варианты через #[serde(other)] fallback
}
```

Координаты — `i32` (сняли legacy `i16`-ограничение Synergy). Протокольная версия только в `Hello` — согласование на handshake; отдельные сообщения не версионируются.

**TLS:** `tokio-rustls`. При первом старте генерируем self-signed cert через `rcgen`. Сохраняем в `<config_dir>/tls/{cert.pem,key.pem}` с правами `0600`. Fingerprint DB — TOML:

```toml
# Список доверенных пиров. Добавляется автоматически после подтверждения
# отпечатка через GUI/CLI или вручную.

[[peer]]
name = "laptop"
fingerprint = "sha256:abc123..."
added = "2026-04-22"
```

**Platform trait (AFIT):**

```rust
pub trait PlatformScreen: Send + Sync {
    fn inject_key(&self, key: KeyId, mods: ModifierMask, down: bool)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_button(&self, btn: ButtonId, down: bool)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_move(&self, x: i32, y: i32)
        -> impl Future<Output = Result<()>> + Send;
    fn inject_mouse_wheel(&self, dx: i32, dy: i32)
        -> impl Future<Output = Result<()>> + Send;
    fn get_clipboard(&self, id: ClipboardId, format: ClipboardFormat)
        -> impl Future<Output = Result<Bytes>> + Send;
    fn set_clipboard(&self, id: ClipboardId, format: ClipboardFormat, data: Bytes)
        -> impl Future<Output = Result<()>> + Send;
    fn screen_info(&self) -> ScreenInfo;
    fn event_stream(&self) -> impl Stream<Item = InputEvent> + Send;
}
```

Там, где потребуется `dyn PlatformScreen` (например, runtime-selection бэкенда), используем отдельный `dyn`-friendly wrapper-трейт на `#[async_trait]`, инкапсулирующий AFIT-версию.

**Wayland/libei:** `reis` крейт за внутренним тонким трейтом `EiBackend` (~3 метода: `create_session`, `poll_events`, `emit_*`). Если `reis` застопорится — swap на `bindgen` — это переписать один `platform/ei/backend.rs`.

**Server routing:** `tokio::sync::mpsc` между задачами: `PlatformReader` → `Router` → `ClientProxy` (по одному на клиента). Screen layout — `Arc<arc_swap::ArcSwap<ScreenLayout>>` для lock-free чтения горячего пути.

**IPC (`ipc` crate):** `interprocess::local_socket` для Unix-domain / Named pipe. Путь:
- Linux: `$XDG_RUNTIME_DIR/hop/daemon.sock`
- macOS: `$TMPDIR/hop/daemon.sock`
- Windows: `\\.\pipe\hop-daemon`

Протокол — newline-delimited JSON в стиле JSON-RPC 2.0 (`{ "id", "method", "params" }` + notifications без `id`). Методы: `get_status`, `reload_config`, `add_peer_fingerprint`, `subscribe_logs`, ...

**Конфигурация (`config` crate):** `figment` оборачивает:
1. Дефолты (hardcoded в крейте)
2. Файл `<config_dir>/config.toml`
3. Env `HOP_*`
4. CLI аргументы (`clap` derive)

Типизированные структуры, валидация на `TryFrom<RawConfig, Error = ConfigError>`.

**Логирование:** `tracing` + `tracing-subscriber` с:
- `fmt` layer → stderr (human / JSON по env-переменной)
- `ipc` layer → сериализует события в IPC-канал для GUI

**Ошибки:** `thiserror` per-crate:

```rust
// protocol/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("CBOR decode failed at byte {offset}: {source}")]
    Decode { offset: u64, source: ciborium::de::Error<std::io::Error> },
    #[error("frame exceeds max size: {size} > {max}")]
    FrameTooLarge { size: usize, max: usize },
    #[error("unsupported protocol version: {got}, expected {expected}")]
    VersionMismatch { got: u16, expected: u16 },
}
```

Бинари используют `anyhow::Result<()>` в `main` + `.context(...)` на стыках.

**Пути:** `directories::ProjectDirs::from("com", "Hop", "hop")`.

**Reconnect:** `backoff` крейт, exponential, 1 с → 30 с max, jitter.

**Таск-менеджмент:** все `tokio::spawn` оборачиваются в `tokio::task::JoinSet`. Паника одной задачи → `tracing::error!` + попытка перезапуска через supervisor pattern; процесс не гасится.

## Edge cases & error handling

- **Несовпадение `protocol_version`:** `Disconnect { reason: ProtocolVersionMismatch { server, client } }`, graceful close.
- **TLS handshake timeout (10 с):** drop соединения, `tracing::warn!` с IP клиента.
- **Fingerprint mismatch:** `Disconnect { reason: UnknownPeer }`, лог fingerprint клиента для ручного добавления в DB. GUI может показать prompt.
- **Платформенный backend недоступен (libei < 1.0, нет X11):** `anyhow::bail!` при старте с конкретной подсказкой (`install libei >= 1.0` / `DISPLAY unset`).
- **CBOR decode error:** `Disconnect { reason: MalformedMessage }`, не паниковать. Лог с hex-дампом первых 64 байт фрейма для отладки.
- **Frame > 16 МБ:** `LengthDelimitedCodec` вернёт ошибку → `Disconnect { reason: FrameTooLarge }`.
- **Clipboard > 1 МБ:** усечь до лимита, `tracing::warn!` с исходным размером.
- **Reconnect loop:** клиент с `backoff`, 1s → 30s max, jitter 0–25%.
- **Tokio task panic:** `JoinSet` с супервизором; падение одной задачи → лог + перезапуск задачи; процесс выживает.
- **IPC socket уже существует:** при старте сервера — удалить stale socket после проверки отсутствия живого процесса (через advisory lock-файл рядом с сокетом).
- **Безопасность IPC:** Unix socket с правами `0600`; Named pipe с ACL на `current_user`.
- **Path traversal (file clipboard):** см. `specs/file-clipboard.md`.

## Порядок реализации

Детальный план milestones — в `specs/milestones/`. Коротко:

| M | Артефакт | Статус подспека |
|---|---|---|
| M0 | Скелет воркспейса, CI, tooling | детально |
| M1 | `protocol` крейт: CBOR-сообщения, property-тесты | детально |
| M2 | `net` крейт: TCP + TLS + handshake + mock screen | детально |
| M3 | `platform/x11`: рабочий сервер+клиент между Linux/X11 | заголовок |
| M4 | Clipboard (текст/HTML) + TOML-конфиг | заголовок |
| M5 | `ipc` + адаптация Qt GUI к новому IPC | заголовок |
| M6 | `platform/ei`: Wayland/libei | заголовок |
| M7 | `platform/macos` | заголовок |
| M8 | `platform/windows` | заголовок |
| M9 | File clipboard (см. `specs/file-clipboard.md`) | заголовок |
| M10 | Windows service обёртка (`hops --service`) | заголовок |

Детальные подспеки для M3–M10 пишутся по мере приближения к milestone.

## Open questions

1. **`#[serde(other)]` fallback для `Capability`:** нужен ли forward-compat для неизвестных capability-вариантов (старый клиент + новый сервер с extra cap)? Рекомендую да — `#[serde(other)] Unknown` вариант, на старом клиенте просто игнорируется.
2. **Screen layout: `arc_swap` vs `tokio::sync::RwLock`:** `arc_swap` быстрее на чтении (lock-free), но reload через GUI редкий. Решить при реализации M3.
3. **`hop-migrate` XML→TOML:** включать в default install или отдельный download? Рекомендую отдельный — не нужен большинству новых пользователей.
