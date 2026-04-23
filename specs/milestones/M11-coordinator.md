# M11 — Server Coordinator

## Цель

Собрать центральный актор сервера, который превращает локальный ввод (на primary) и сетевые сообщения от клиентов в правильный поток `Message`-ов. Сейчас `Server::serve` делает accept + keep-alive и только. Нет пути "локальная клавиатура → клиент", нет переключения активного экрана, нет обработки clipboard-протокола. M11 закрывает этот пробел.

## Scope

**В области:**
- Структура `Coordinator` — pure state-machine, owning screen layout, активный экран, курсор, удерживаемые клавиши/кнопки/модификаторы, clipboard-grab-state, registry of connected clients.
- Edge-crossing механика (rect-based virtual layout) с корректным освобождением/повторным нажатием модификаторов.
- Маршрутизация `InputEvent` от локального `PlatformScreen::event_stream()` в нужный `ClientProxy`.
- Приём входящих `Message`-ов от клиентов (`CoordinatorEvent::PeerMessage`) и их обработка (clipboard protocol, disconnect).
- Интеграция с существующими `Server::serve` / `ClientProxy` через channels.
- Shutdown propagation: `CancellationToken` → все клиентские таски.

**Out of scope:**
- Lazy-clipboard (платформа отдаёт данные по требованию при Ctrl+V на локальной стороне). Требует нового API в `PlatformScreen`, делается в M11.1.
- Сценарий "Ctrl+V на primary после того как secondary захватил буфер". Работает только при наличии lazy-clipboard. Пока documented limitation.
- Drag-across-edge (когда мышь тянет кнопку и пересекает границу). Mouse crossing блокируется пока зажата хоть одна кнопка — стандартная Barrier/Synergy семантика.
- GUI reload_config: `arc_swap` выбран именно ради будущего swap, но сам механизм reload — отдельный PR.
- File-clipboard (M9) через Coordinator: transfer engine уже есть; маршрутизация `FileTransferStart/Chunk/End/Cancel` через Coordinator — тривиальный add-on, но в этом milestone не оформляется отдельной фичей.

## Архитектура

### Компонентная диаграмма

```
┌──────────────────┐        InputEvent           ┌───────────────────┐
│  PlatformScreen  │ ──────────────────────────▶ │                   │
│  (event_stream)  │                             │                   │
└──────────────────┘                             │                   │
                                                 │                   │
┌──────────────────┐  ClientEvent::Connected /   │                   │
│  Server accept   │  ::Disconnected /           │   Coordinator     │
│  loop            │ ─────PeerMessage──────────▶ │   task            │
└──────────────────┘                             │                   │
                                                 │                   │
┌──────────────────┐   CancellationToken         │                   │
│  SIGINT handler  │ ──────────────────────────▶ │                   │
└──────────────────┘                             └────────┬──────────┘
                                                          │
                            ┌─────────────────────────────┤
                            │ per-client mpsc<Message>    │
                            ▼                             ▼
                    ┌───────────────┐            ┌───────────────┐
                    │ ClientProxy   │            │ ClientProxy   │
                    │ "laptop"      │            │ "desktop"     │
                    └───────┬───────┘            └───────┬───────┘
                            │ framed TLS                 │
                            ▼                            ▼
                        laptop peer                 desktop peer
```

### Слои и файлы

```
crates/server/src/
├── lib.rs               # re-exports, Server::bind/serve (wires it all together)
├── error.rs             # ServerError (unchanged from M10.5 fix #1)
├── coordinator/
│   ├── mod.rs           # Coordinator struct + Event / Output enums
│   ├── layout.rs        # ScreenLayout (rect-based virtual space) + crossing math
│   ├── held.rs          # HeldState (keys/buttons/mods) + transition helpers
│   ├── clipboard.rs     # ClipboardGrabState (owner/seq tracking, pending requests)
│   └── task.rs          # The tokio task that drives Coordinator + channel plumbing
└── proxy.rs             # ClientProxy: framed<->mpsc adapter + keep-alive
```

## Data shapes

### ScreenLayout (rect-based)

Каждый экран — прямоугольник в виртуальном координатном пространстве. Курсор живёт в глобальных координатах; active-screen определяется тем, в какой rect он попал.

```rust
pub struct ScreenLayout {
    screens: Vec<ScreenEntry>,
    primary: ScreenName,
}

pub struct ScreenEntry {
    pub name: ScreenName,
    /// Top-left corner of this screen in the virtual coordinate space.
    pub origin_x: i32,
    pub origin_y: i32,
    /// Physical resolution in pixels.
    pub width: u32,
    pub height: u32,
}

pub type ScreenName = String;  // matches display_name from Hello
```

Пример конфига (TOML в будущем; структура фиксируется сейчас):
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
origin_y = 90      # vertical centering
width = 1440
height = 900

[[screen]]
name = "monitor"
origin_x = 1920    # to the right of desk
origin_y = -180    # top alignment differs
width = 2560
height = 1440
```

Query: `layout.screen_at(vx, vy) -> Option<&ScreenEntry>` — O(n), где n — число экранов (обычно ≤ 5).

**Решения по развилке:**
- Layout в `Arc<ArcSwap<ScreenLayout>>`. Coordinator загружает snapshot один раз за `on_input`-итерацию через `.load()`. Lock-free read path.

### LayoutStore + где хранится layout на диске

Layout живёт в **отдельном файле** `layout.toml`, не внутри основного `config.toml`. Причины:

1. **Разные владельцы:** `config.toml` — admin-level (`listen_addr`, `cert_dir`), редко редактируется; `layout.toml` — user-level, редактируется GUI каждый раз когда пользователь перестраивает мониторы. Отдельные файлы → GUI может делать atomic write (`tempfile` + rename) без риска снести admin-настройки и комментарии.
2. **Precedent:** `FingerprintDb` уже в своём файле (`fingerprints.toml`); архитектурно мы уже выбрали "то, что часто меняется извне — в отдельном файле".
3. **Живой reload:** `ArcSwap` выбран именно ради hot-swap. Отдельный файл позволяет reload'у не тянуть за собой figment-слои (env, CLI), которые к layout не относятся.

**Структура:**

```toml
# ~/.config/input-leap/config.toml

listen_addr = "0.0.0.0:24800"
display_name = "desk"

[tls]
cert_dir       = "./config/tls"
fingerprint_db = "./config/fingerprints.toml"

[layout]
# Path to the layout file. Reloaded live via IPC reload_layout().
path = "./config/layout.toml"
```

```toml
# ~/.config/input-leap/layout.toml

primary = "desk"

[[screen]]
name     = "desk"
origin_x = 0
origin_y = 0
width    = 1920
height   = 1080

[[screen]]
name     = "laptop"
origin_x = -1440
origin_y = 90
width    = 1440
height   = 900
```

**Только `ServerSettings` получает `LayoutSettings { path: PathBuf }`.** Клиент layout не читает — у него всегда один экран.

**Default path:** `<project_config_dir>/layout.toml` через `directories::ProjectDirs` — рядом с `config.toml`.

**Отсутствие файла:** `tracing::warn!("layout file not found at {}; add at least one client screen to route input", path)` + пустой layout (только primary). Сервер стартует в degraded mode (никуда не роутит), но не падает — важно для first-run UX, когда пользователь ещё не настроил layout.

**`LayoutStore` — тонкая обёртка:**

```rust
pub struct LayoutStore {
    path: PathBuf,
    inner: Arc<ArcSwap<ScreenLayout>>,
}

impl LayoutStore {
    /// Load from disk. Missing file → empty layout + warn.
    pub fn load(path: PathBuf) -> Result<Self, ConfigError>;
    /// Cheap: Arc pointer-bump.
    pub fn snapshot(&self) -> Arc<ScreenLayout>;
    /// Re-read from disk and atomically swap. Coordinator sees the
    /// new layout on its next on_event iteration.
    pub fn reload(&self) -> Result<(), ConfigError>;
}
```

GUI дёргает `reload_layout` через IPC (новый метод в `IpcHandler`, добавляется в M11 wire-up) → сервер вызывает `store.reload()` → `Coordinator` следующим `on_event` видит новый layout.

### HeldState

```rust
pub struct HeldState {
    keys: BTreeSet<KeyId>,           // non-modifier keys held on active side
    buttons: BTreeSet<ButtonId>,     // mouse buttons held on active side
    mods: ModifierMask,              // modifiers (Shift/Ctrl/Alt/Meta/Locks/AltGr)
}

impl HeldState {
    /// Apply one event to the held state. Returns `true` if modifier
    /// mask changed as a side-effect (helps callers decide whether to
    /// re-emit mods on a screen transition).
    pub fn apply(&mut self, event: &InputEvent) -> bool;

    /// Messages needed to "unstick" the currently-held state on the
    /// active screen before we leave it.
    pub fn leave_messages(&self) -> Vec<Message>;

    /// Messages needed to "restore" held modifiers on the new active
    /// screen after we entered it. Non-modifier keys and buttons are
    /// intentionally NOT re-pressed (see design rationale).
    pub fn enter_messages(&self) -> Vec<Message>;

    pub fn any_button_held(&self) -> bool;
}
```

**Решения по развилке 3 (где живёт held-state):** внутри `Coordinator` на основе потока `InputEvent`. Платформенный слой отдаёт сырые KeyDown/KeyUp/MouseButton; Coordinator агрегирует. Это позволяет Coordinator'у корректно "размотать" состояние при переходе.

**Решения по re-press policy:**
- На `leave_messages()`: отдаём `KeyUp` для каждого элемента в `keys` и `ButtonUp` (через `MouseButton { down: false }`) для каждого в `buttons`. Потом modifiers зануляются через отдельные `KeyUp`-события для каждого флага в `mods`.
- На `enter_messages()`: отдаём `KeyDown` **только** для модификаторов из `mods`. Не-модификаторные клавиши и кнопки **не** переносятся (редкий случай, безопаснее отпустить и забыть).

**Решения по drag-across-edge:** если `any_button_held() == true`, то `Coordinator::on_input(MouseMove)` **не выполняет пересечение границы**; курсор клампится к текущему экрану до тех пор, пока все кнопки не отпущены. Match Barrier/Synergy behavior.

### ClipboardGrabState

```rust
pub struct ClipboardGrabState {
    /// Current owner per clipboard id (Clipboard / Primary).
    owner: HashMap<ClipboardId, GrabRecord>,
}

pub struct GrabRecord {
    pub owner: ScreenName,
    /// Monotonic seq bumped on screen-transitions; peers use it to
    /// discard stale Grab/Request messages that arrived after the
    /// active screen moved on.
    pub seq: u32,
}

impl ClipboardGrabState {
    pub fn current_seq(&self, id: ClipboardId) -> u32;
    pub fn bump_seq(&mut self, id: ClipboardId);
    pub fn on_grab(&mut self, from: ScreenName, id: ClipboardId, seq: u32) -> bool;  // true if accepted
    pub fn owner_of(&self, id: ClipboardId) -> Option<&ScreenName>;
}
```

Coordinator держит один экземпляр; модуль тестируется отдельно.

### Coordinator

```rust
pub struct Coordinator {
    layout: Arc<ArcSwap<ScreenLayout>>,
    active: ScreenName,                          // whose inputs are being captured / forwarded
    cursor: (i32, i32),                          // virtual coords
    held: HeldState,
    grabs: ClipboardGrabState,
    clients: HashMap<ScreenName, ClientHandle>,  // connected + known-in-layout
    orphans: HashMap<ScreenName, ClientHandle>,  // connected but not in layout
    seq: u32,
}

pub struct ClientHandle {
    pub tx: mpsc::Sender<Message>,
    pub capabilities: Vec<Capability>,
}

pub enum CoordinatorEvent {
    /// Local platform input (primary side only).
    LocalInput(InputEvent),
    /// A peer connected and finished handshake.
    ClientConnected {
        name: ScreenName,
        tx: mpsc::Sender<Message>,
        capabilities: Vec<Capability>,
    },
    /// A peer disconnected for any reason.
    ClientDisconnected { name: ScreenName },
    /// A peer sent us a wire message.
    PeerMessage { from: ScreenName, msg: Message },
    /// Layout has been swapped (future: reload_config).
    LayoutReloaded,
}

pub enum CoordinatorOutput {
    /// Send a message to a specific client.
    Send { to: ScreenName, msg: Message },
    /// Inject a message locally (only meaningful when `active == primary`).
    InjectLocal(Message),
    /// Log + metrics hook.
    Warn(String),
}

impl Coordinator {
    pub fn new(
        layout: Arc<ArcSwap<ScreenLayout>>,
        local: ScreenName,  // the primary's own name
    ) -> Self;

    /// Single entry point. `buf` is reused by callers to avoid per-event allocation.
    pub fn on_event(&mut self, event: CoordinatorEvent, buf: &mut Vec<CoordinatorOutput>);
}
```

**Решения по развилке 5 (pure vs side-effectful):** pure + `Vec<Output>`. Caller reuses one `Vec<CoordinatorOutput>` buffer across calls. Tests feed events, assert outputs — no tokio runtime needed for 95% of the test matrix.

## Ключевые инварианты + порядок операций

### При MouseMove от локального event_stream

Последовательность внутри Coordinator:
1. Обновить `cursor += (dx, dy)` (для RelMove) или `cursor = (x, y)` (для абсолютного).
2. `if self.held.any_button_held() { clamp cursor to current active screen rect; emit forward-as-usual; return; }`
3. `layout.screen_at(cursor)` — найти в каком экране сейчас.
4. Если это то же что `self.active` → просто форвардим MouseMove в соответствующий поток (локально injected или сеть).
5. Если другой экран → **атомарная транзакция пересечения**:
   - `for msg in held.leave_messages()` → `Send` to old active (если remote) / `InjectLocal` (если primary).
   - `Send ScreenLeave` to old active (если remote).
   - `self.active = new_name`
   - `self.seq += 1` (глобальный seq для ScreenEnter)
   - `Send ScreenEnter { x, y, seq, mask = self.held.mods }` to new active (если remote).
   - `for msg in held.enter_messages()` → `Send` to new active.
6. После transition — пропускаем оригинальный MouseMove в новый active (с локальными координатами внутри target screen).

### При Key/Button events

1. `self.held.apply(event)` — обновить множества.
2. Если `active == local_primary` — `InjectLocal` (noop для сервера, событие уже произошло локально). По сути: не нужно ничего делать, пользователь видит результат локально.
3. Если `active` — remote client — `Send { to: active, msg: Message::Key/Button(...) }`.

### При ClipboardGrab от клиента

`Coordinator::on_event(PeerMessage { from, ClipboardGrab { id, seq }})`:
1. `self.grabs.on_grab(from, id, seq)` — если `seq < current_seq`, игнорируем (stale).
2. Broadcast `ClipboardGrab { id, seq }` всем клиентам **кроме** `from`.
3. Нет платформенных действий (lazy-clipboard — out of scope).

### При ClipboardRequest от клиента

`Coordinator::on_event(PeerMessage { from, ClipboardRequest { id, seq }})`:
1. Look up owner. Если это primary — платформа через отдельный путь (см. "Локальный path к платформе" ниже). Пока что: `Warn("clipboard request for primary not supported yet")`.
2. Если это другой client — `Send { to: owner, msg: ClipboardRequest { id, seq }}`.

### При ClipboardData от клиента

`Coordinator::on_event(PeerMessage { from, ClipboardData { id, format, data }})`:
1. Если у нас есть outstanding request для этого `(id, seq)` — forward `ClipboardData` к requester.
2. Иначе warn + drop.

### При ClientConnected / ClientDisconnected

Connected:
1. Если `name` есть в `layout` → `clients.insert(name, handle)`.
2. Иначе → `orphans.insert(name, handle)` + `Warn("client 'X' connected but not in layout; inputs won't be routed to it")`.
3. Bootstrap: `Send ScreenEnter` with seq=0 если этот клиент сразу становится активным (редко; обычно primary стартует активным).

Disconnected:
1. `clients.remove(name)` / `orphans.remove(name)`.
2. Если `active == name` → switch `active` back to `primary`, `self.seq += 1`, и emit `held.enter_messages()` как локальную инжекцию (технически noop, но seq-bump корректен для будущих clipboard-grab'ов).
3. Если `name` был owner какого-либо clipboard → очистить соответствующие `grabs.owner.remove(...)`.

## ClientProxy

```rust
pub struct ClientProxy {
    name: ScreenName,
    framed: HandshakeStream,
    inbound_tx: mpsc::Sender<CoordinatorEvent>,  // wraps PeerMessage
    outbound_rx: mpsc::Receiver<Message>,
    shutdown: CancellationToken,
}

impl ClientProxy {
    pub async fn run(self) -> Result<(), ServerError>;
}
```

Цикл:
```rust
loop {
    select! {
        biased;
        () = shutdown.cancelled() => {
            framed.send(Message::Disconnect { reason: UserInitiated }).await.ok();
            break;
        }
        Some(msg) = outbound_rx.recv() => {
            framed.send(msg).await?;
        }
        incoming = framed.next() => {
            match incoming {
                Some(Ok(msg)) => {
                    keepalive.mark_seen();
                    match msg {
                        Message::Disconnect { .. } => break,
                        msg => {
                            inbound_tx.send(CoordinatorEvent::PeerMessage { from: name.clone(), msg })
                                .await
                                .map_err(|_| ServerError::CoordinatorGone)?;
                        }
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
        _ = keepalive.tick() => {
            if keepalive.is_timed_out() {
                framed.send(Message::Disconnect { reason: KeepAliveTimeout }).await.ok();
                break;
            }
            framed.send(Message::KeepAlive).await?;
        }
    }
}
// on exit: send Disconnected event to coordinator
inbound_tx.send(CoordinatorEvent::ClientDisconnected { name }).await.ok();
```

**Решение по развилке 4 (backpressure на outbound):**
- `mpsc::channel(1024)` bounded.
- `Coordinator` в `task.rs` использует `tx.try_send(msg)`:
  - `Ok(())` — штатно.
  - `Err(TrySendError::Full(_))` — медленный клиент → закрываем connection: посылаем `ClientDisconnected` через loopback канал, Coordinator удаляет клиента, прокси-task ловит это через падение outbound канала и завершается. `Warn("client X dropped due to outbound backpressure")`.
  - `Err(TrySendError::Closed(_))` — прокси уже ушёл, тихо удаляем из `clients`.

## Локальный path к платформе

`Coordinator` не вызывает `PlatformScreen` напрямую (иначе становится неприемлемо сложно тестировать). Вместо этого `task.rs`, который гоняет Coordinator, имеет **второй outbound канал** `mpsc<Message>` → `PlatformDispatcher` задача. Эта задача:
- Берёт `Message::Key/Mouse/Clipboard*` → вызывает `screen.inject_key(...)` / `screen.set_clipboard(...)`.
- Для primary-side `InjectLocal` — это именно то, что происходит когда `active == local`, но обычно локально не нужно (OS уже обработала). Полезно для clipboard set'а (когда хотим записать данные в локальный буфер при входящем ClipboardData от другого peer'а).

## Shutdown propagation

```
CancellationToken (SIGINT)
    ├─▶ Server::serve loop exits
    ├─▶ Coordinator task: drains its inbound channel, does last `on_event(ClientDisconnected)` for every remaining client, sends CoordinatorEvent::Shutdown internally which makes task exit.
    ├─▶ Each ClientProxy: biased select catches cancelled() first, sends Disconnect to peer, closes.
    └─▶ PlatformDispatcher: drains, exits.
```

`JoinSet` на всех ClientProxy + на Coordinator task + на PlatformDispatcher, awaited в `Server::serve` epilogue.

## Порядок имплементации

1. ✅ **`coordinator/layout.rs`** (commit `54d7f6b`) — `ScreenLayout`, `ScreenEntry`, `screen_at()`, `clamp()`, `LayoutStore` c `ArcSwap` + live reload. 11 unit-тестов.

2. ✅ **`coordinator/held.rs`** (commit `54d7f6b`) — `HeldState::{apply, leave_messages, enter_messages, any_button_held}`. 10 unit-тестов (Shift replay, drag block, modifier fixed-order).

3. ✅ **`coordinator/clipboard.rs`** (commit `54d7f6b`) — `ClipboardGrabState` с seq-based stale-detection. 6 unit-тестов.

4. ✅ **`coordinator/state.rs`** (commit `63c0b0b`) — `Coordinator`, `CoordinatorEvent`, `CoordinatorOutput`. Pure state-machine. 11 unit-тестов (crossing, drag-block, orphan, active-disconnect, clipboard broadcast/request/stale).

5. ✅ **`coordinator/proxy.rs`** — `ClientProxy` с outbound mpsc + keep-alive + inbound forward. 5 интеграционных тестов на `tokio::io::duplex` (PeerMessage forwarding, outbound writes, keep-alive filter, peer-disconnect, coordinator-drop).

6. ✅ **`coordinator/task.rs`** — tokio driver task + platform dispatcher. `try_send` backpressure drop-on-full. 3 unit-теста (crossing emits ScreenEnter, backpressure tolerance, InjectLocal reaches dispatcher).

7. ✅ **`Server::serve`** — переписан в `crates/server/src/lib.rs`: `spawn_coordinator` + input-stream forwarder + per-peer `ClientProxy`. `ServerConfig` получил обязательное поле `layout: SharedLayout`; binary (`bins/input-leaps/src/main.rs`) пока подставляет `ScreenLayout::single_primary(display_name)` до появления loader'а `layout.toml`.

8. ✅ **E2E test:** `crates/server/tests/coordinator_e2e.rs` — 3-screen layout, два mock-клиента, MouseMove через desk → monitor → laptop. Ассертим что monitor получает ScreenEnter + MouseMove + ScreenLeave, laptop получает ScreenEnter без ScreenLeave.

**Текущий статус:** все 8 шагов готовы. 49 server-тестов + 1 handshake E2E + 1 coordinator E2E, clippy clean.

## Тестовый план

| Уровень | Что | Чем |
|---|---|---|
| Unit | ScreenLayout: `screen_at`, rect-arithmetic | `proptest` — random rects, random points |
| Unit | HeldState leave/enter symmetry | `rstest` — параметризованные зажатия |
| Unit | ClipboardGrabState state machine | ручные unit-тесты |
| Unit | Coordinator::on_event — вся матрица событий | ручные unit-тесты, по одной на сценарий |
| Integration | ClientProxy inbound/outbound через mock TCP | `tokio::net::duplex` |
| E2E | Server с двумя mock-клиентами + симулированным event_stream | через `MockScreen` + `Server::bind/serve` |

Планируемое покрытие: ≥ 85% для `coordinator/` модуля (чистый логический код). ClientProxy `tokio`-зависим, ~70% достаточно.

## Оценка и риски

- **Разработка:** ~3 дня. Из них ~1 день на layout + held + clipboard модули, ~1 день на Coordinator + tests, ~1 день на task.rs + ClientProxy + E2E.
- **Риски:**
  - Edge-crossing математика легко получает off-by-one в граничных точках (курсор ровно на границе, выход за угол экрана). Покроем property-тестами.
  - Backpressure drop-on-full может оказаться слишком агрессивным на slow networks. Если в E2E будут flaky тесты — переключимся на bounded(8192) для буферизации коротких всплесков.
  - Lazy-clipboard defer'ится — это значит Ctrl+V на primary после remote grab не работает. Документировать в README.

## Resolved / deferred decisions

1. ~~**Layout storage.**~~ **Resolved:** отдельный `layout.toml`, путь конфигурируется через `ServerSettings.layout.path`. Причины — в разделе "LayoutStore + где хранится layout на диске" выше.

2. **Scale-factor inter-screen:** DPI 100% на laptop vs 150% на desk. При пересечении границы y-координата должна масштабироваться? Текущий rect-based layout работает в физических пикселях. **Deferred:** heterogeneous-DPI проигнорировать до появления второго реального клиента; тогда добавим `logical_height` / трансформацию координат. На момент M11 работаем в физических пикселях всех экранов.

3. ~~**Broadcast vs per-client seq.**~~ **Resolved:** один **глобальный** `self.seq` на весь Coordinator. Альтернатива per-client seq рассматривалась — отклонена, потому что переключение активного экрана — event, относящийся ко всем клиентам сразу (clipboard grab в момент cross должен иметь один seq, видимый одинаково для каждого peer'а). Менять не планируется.
