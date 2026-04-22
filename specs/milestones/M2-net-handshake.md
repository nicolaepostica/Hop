# M2 — `net` crate: TCP + TLS + handshake + MockScreen

## Цель

Получить работающий end-to-end канал «Rust-сервер ↔ Rust-клиент» через TCP + TLS с полным handshake и обменом keep-alive, без настоящего платформенного слоя (MockScreen). По завершении M2 можно запустить `input-leaps` и `input-leapc` на localhost, они пройдут рукопожатие, обменяются DeviceInfo и будут гонять KeepAlive до `Ctrl+C`.

## Предпосылки

- [M0](M0-skeleton.md) — воркспейс
- [M1](M1-protocol.md) — `protocol` крейт

## Scope

**In scope:**
- `net` крейт: `Listener`, `ConnectedStream`, абстракция TCP+TLS
- Self-signed cert generation через `rcgen` при первом старте
- Загрузка / сохранение cert + key в `<config_dir>/tls/`
- Fingerprint DB loader (TOML), базовый CRUD
- Handshake state machine: TLS handshake → `Hello` exchange → `DeviceInfo` exchange → active
- KeepAlive timer через `tokio::time::interval`, 3 пропуска = disconnect
- MockScreen в `platform/core` для тестирования без реального backend
- Minimal CLI для `input-leaps` и `input-leapc`: `--listen`, `--connect`, `--fingerprint`, `--name`
- Integration test: два `tokio::spawn` процесса-эмулятора прогоняют полный handshake + 5 секунд KeepAlive
- Graceful shutdown через `tokio::signal::ctrl_c` + `Disconnect { reason: UserInitiated }`

**Out of scope:**
- Настоящий платформенный ввод/вывод (M3+)
- Clipboard (M4)
- IPC с GUI (M5)
- Конфиг-файл TOML (M4) — пока используем только CLI аргументы
- Hot-reload сертификата / fingerprint DB (будущее, не в MVP)

## Задачи

### `net` crate — transport

- [ ] `crates/net/src/tls.rs`:
  - `pub struct TlsConfig { server_config, client_config, fingerprint_db }`
  - `fn load_or_generate_cert(dir: &Path) -> Result<(Certificate, PrivateKey)>`:
    - Если `cert.pem` и `key.pem` в `dir` есть — загрузить через `rustls-pemfile`
    - Иначе — сгенерировать через `rcgen::generate_simple_self_signed(vec!["input-leap-host".into()])`, сохранить с правами `0600` на Unix / ACL на Windows
  - Custom `rustls::server::ClientCertVerifier` и `rustls::client::ServerCertVerifier` — verify через fingerprint DB (не через CA chain)
- [ ] `crates/net/src/fingerprint.rs`:
  - `struct FingerprintDb` — wrapper над `Vec<PeerEntry>`
  - `struct PeerEntry { name: String, fingerprint: Fingerprint, added: chrono::DateTime<Utc> }`
  - `struct Fingerprint([u8; 32])` (SHA-256) с `Display`/`FromStr` (формат `sha256:hex`)
  - `fn load(path: &Path) -> Result<FingerprintDb>` — TOML через `toml` crate
  - `fn save(&self, path: &Path) -> Result<()>`
  - `fn contains(&self, fp: &Fingerprint) -> Option<&PeerEntry>`
- [ ] `crates/net/src/listener.rs`:
  - `pub struct Listener { tcp: TcpListener, tls: Arc<ServerConfig> }`
  - `async fn accept(&self) -> Result<ConnectedStream>` — accept TCP → TLS handshake → возвращает готовый `ConnectedStream`
  - TLS handshake в отдельной spawned task с timeout 10 с
- [ ] `crates/net/src/client.rs`:
  - `pub async fn connect(addr: SocketAddr, tls: Arc<ClientConfig>) -> Result<ConnectedStream>`
- [ ] `crates/net/src/stream.rs`:
  - `pub struct ConnectedStream` — обёртка над `tokio_rustls::TlsStream<TcpStream>`
  - Методы доступа к peer fingerprint (вытаскиваем из `CertificateDer` через `rustls-pki-types`)
  - `into_framed(self) -> Framed<..., MessageCodec>` — мостик к `protocol`

### Handshake state machine

- [ ] `crates/net/src/handshake.rs`:
  ```rust
  pub struct HandshakeResult {
      pub peer_name: String,
      pub peer_capabilities: Vec<Capability>,
      pub peer_device_info: DeviceInfoPayload,
  }

  pub async fn server_handshake(
      conn: &mut Framed<ConnectedStream, MessageCodec>,
      our_info: &HelloPayload,
      our_device: &DeviceInfoPayload,
  ) -> Result<HandshakeResult, HandshakeError>;

  pub async fn client_handshake(...) -> Result<HandshakeResult, HandshakeError>;
  ```
- Шаги (сервер):
  1. Ждём `Hello` от клиента с timeout 5 с
  2. Валидируем `protocol_version == 1`
  3. Шлём свой `Hello`
  4. Шлём `DeviceInfoRequest`
  5. Ждём `DeviceInfo` с timeout 5 с
  6. Возвращаем `HandshakeResult`
- Клиент — симметрично (инициирует `Hello` первым, отвечает на `DeviceInfoRequest`)
- Ошибки: `HandshakeError` через `thiserror` с вариантами для каждой фазы

### KeepAlive

- [ ] `crates/net/src/keepalive.rs`:
  - `pub struct KeepAliveTask { tx: mpsc::Sender<Message>, last_seen: Arc<AtomicU64> }`
  - `spawn_keepalive(tx, last_seen)` — `tokio::time::interval(3s)` шлёт `KeepAlive` + проверяет `last_seen` — если > 9 с, шлёт `Disconnect { reason: KeepAliveTimeout }` и завершается
  - Входящие `KeepAlive` просто обновляют `last_seen` (атомарно)

### MockScreen

- [ ] `crates/platform/core/src/mock.rs`:
  - `pub struct MockScreen { events: Mutex<Vec<RecordedEvent>>, ... }`
  - Полная реализация `PlatformScreen` — записывает все `inject_*` в in-memory лог, `event_stream` возвращает заранее загруженный `Vec<InputEvent>`
  - Используется в тестах M2 (для тестов server/client) и далее до M3

### Server / Client минимальные main'ы

- [ ] `crates/server/src/lib.rs`:
  - `pub async fn run(config: ServerConfig, screen: impl PlatformScreen) -> Result<()>` — accept loop + per-client task
  - Каждое входящее соединение → handshake → цикл `select!` { incoming Message | keepalive | shutdown }
  - На M2: получает `MouseMove`/`KeyDown` — просто логирует через `tracing::info!`, не инжектит никуда (это M3+)
- [ ] `crates/client/src/lib.rs`:
  - `pub async fn run(config: ClientConfig, screen: impl PlatformScreen) -> Result<()>` — connect + handshake + event loop
  - Событийная часть (screen.event_stream) пока отдаёт в пустоту (MockScreen возвращает empty stream)
- [ ] `bins/input-leaps/src/main.rs`:
  - `clap` derive с флагами `--listen 0.0.0.0:24800`, `--name`, `--cert-dir`, `--fingerprint-db`
  - `tracing_subscriber::fmt().init()`
  - Создаёт `MockScreen`, вызывает `server::run`
  - Корректный shutdown на `SIGINT`/`Ctrl+C`
- [ ] `bins/input-leapc/src/main.rs`:
  - Аналогично, с `--connect 127.0.0.1:24800`, `--server-fingerprint`, `--name`

### Тесты

- [ ] `crates/net/tests/handshake.rs`:
  - `#[tokio::test]` поднимает `Listener` на `127.0.0.1:0` (random port) и коннектится через `connect`
  - Проходит полный handshake, проверяет `HandshakeResult` с обеих сторон
  - Проверяет, что `DeviceInfo` передаётся корректно
- [ ] `crates/net/tests/handshake_failures.rs`:
  - TLS timeout: клиент коннектится, но не шлёт TLS handshake — сервер drop через 10 с
  - Hello timeout: TLS прошёл, но клиент не шлёт `Hello` — сервер drop через 5 с
  - Wrong protocol_version: клиент шлёт `Hello { protocol_version: 999 }` — сервер шлёт `Disconnect { reason: ProtocolVersionMismatch }` и закрывает
  - Unknown fingerprint: клиент с неизвестным fingerprint — сервер rejects в verifier, соединение не устанавливается
- [ ] `crates/net/tests/keepalive.rs`:
  - Два peer'а, один перестаёт слать `KeepAlive` (эмуляция через mock) — второй отваливается с `KeepAliveTimeout` за ~9 с (в тесте используем `tokio::time::pause/advance` для детерминизма)
- [ ] `tests/e2e.rs` (на уровне workspace, не конкретного крейта):
  - Запускает `input_leap_server::run` и `input_leap_client::run` как две `tokio::spawn` задачи на random ports
  - С помощью MockScreen проверяет что handshake проходит, 3 KeepAlive циклически обмениваются, затем graceful shutdown через cancellation token
  - Таймаут теста — 15 секунд

### Fingerprint DB CRUD

- [ ] CLI subcommand `input-leaps fingerprint add <name> <fp>` и `input-leaps fingerprint list`
- [ ] Формат файла:
  ```toml
  # <config_dir>/fingerprints.toml
  [[peer]]
  name = "laptop"
  fingerprint = "sha256:abcdef..."
  added = "2026-04-22T10:00:00Z"
  ```

### Логирование

- [ ] На handshake — `tracing::info!(peer = %name, fingerprint = %fp, "peer connected")`
- [ ] На disconnect — `tracing::info!(peer = %name, reason = ?r, "peer disconnected")`
- [ ] На ошибки — `tracing::warn!` или `error!` в зависимости от severity
- [ ] `RUST_LOG=input_leap=debug` — управление через env

## Acceptance criteria

- [ ] `cargo run --bin input-leaps -- --listen 127.0.0.1:24800 --name server-a` стартует, слушает порт, пишет `fingerprint: sha256:...` в лог
- [ ] `cargo run --bin input-leapc -- --connect 127.0.0.1:24800 --server-fingerprint sha256:... --name client-b` коннектится, handshake проходит, оба печатают `peer connected`
- [ ] На `Ctrl+C` с любой стороны — graceful disconnect, оба процесса выходят с code 0
- [ ] Интеграционный тест e2e проходит в CI (Linux/macOS/Windows) менее чем за 15 секунд
- [ ] Все unit/integration тесты зелёные
- [ ] Clippy `-D warnings` зелёный
- [ ] Добавлен `CHANGELOG.md` с записью «M2: TLS handshake + KeepAlive»

## Тесты

Все описаны в разделе «Тесты» выше. Для детерминизма KeepAlive-тестов используем `tokio::time::pause` / `advance` вместо реального `sleep`. Для сетевых тестов — `127.0.0.1:0` (random port) + `tokio::net::lookup_host`.

## Риски / open questions

1. **`rustls` custom verifier API:** меняется между мажорными версиями `rustls`. Зафиксировать точную версию в workspace deps и readme. План: использовать `rustls::server::danger::ClientCertVerifier` (с `danger` feature) — это правильный путь для self-signed + fingerprint model.
2. **Fingerprint DB race condition:** если GUI и daemon одновременно пишут fingerprints.toml — конфликт. В M2 не решаем (только daemon пишет); в M5 — через IPC-команду `add_peer_fingerprint` (единственный writer — daemon).
3. **CN/SAN в self-signed cert:** что ставить? Предлагаю `input-leap-<random-suffix>` как SAN и DNS-имя. Verifier всё равно смотрит только на fingerprint, не на CN.
4. **Windows cert storage permissions:** `0600` эквивалент через `windows-acl` или `windows-rs`. Первично — файл в user profile dir (уже изолирован ОС), acl опционально как hardening.
5. **Graceful shutdown двух сторон:** кто инициирует `Disconnect`? — тот, кто получил `Ctrl+C`. Другая сторона видит `Disconnect` → закрывает stream → выход из event-loop. Проверить в e2e тесте.
6. **`bytes::Bytes` во `ClipboardData` vs `Vec<u8>`:** для M2 неважно (clipboard в M4); упомянуто здесь, чтобы не забыть при проектировании `ClipboardData` в M1.

## Готовность к M3

После M2 у нас есть:
- Работающий TCP+TLS+handshake поверх `MockScreen`
- Полный цикл lifecycle соединения
- Fingerprint-based trust model

M3 добавляет настоящий `platform/x11` backend — `MockScreen` заменяется на `X11Screen`, всё остальное не трогается.
