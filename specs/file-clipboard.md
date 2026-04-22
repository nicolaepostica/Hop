# Spec: File Clipboard — передача файлов через буфер обмена

## Цель

Расширить общий буфер обмена Input Leap так, чтобы при копировании файлов или папок в файловом менеджере (Ctrl+C) и последующей вставке (Ctrl+V) на другом компьютере файлы автоматически передавались по сети и появлялись в целевой папке назначения. Поддержка нескольких файлов и рекурсивных папок.

## Background

Rust-версия Input Leap (см. `specs/rust-rewrite.md`) изначально поддерживает только текст и HTML в clipboard. Этот спек добавляет файловый clipboard как capability-расширение протокола v1.

На уровне ОС файлы в clipboard представлены:
- **Windows:** `CF_HDROP` — список путей + `DROPFILES` структура
- **Linux/X11:** MIME-тип `text/uri-list` — `file:///path\r\n`-разделённый список URI
- **macOS:** `NSFilenamesPboardType` / `public.file-url` — массив путей в pasteboard
- **Wayland:** `text/uri-list` через `wl_data_device` / `ext-data-control`

## Scope

**In scope:**
- Новый capability `Capability::FileClipboard` для handshake
- Определение MIME-подобного формата `ClipboardFormat::Files` с типизированным содержимым
- Обнаружение файлового clipboard на каждой платформе и чтение списка путей/URI
- Передача дерева файлов (рекурсивно) через сетевой протокол от сервера к клиенту и симметрично
- Запись принятых файлов в настраиваемую папку назначения (drop directory)
- Прогресс-уведомления через IPC в GUI обеим сторонам (отправителю и получателю)
- Поддержка Windows, macOS, Linux/X11, Linux/Wayland

**Out of scope:**
- Drag & drop (отдельная фича, после MVP)
- Синхронизация изменений файлов в реальном времени
- Разрешение конфликтов имён с диалогом (v1: автосуффикс `_1`, `_2`)
- Передача символических ссылок (в v1 skip с warn)
- Сохранение permissions/xattr/ACL — в v1 только содержимое и имена

## Requirements

1. Ctrl+C на одном или нескольких файлах/папках в файловом менеджере → Ctrl+V на другой машине → все файлы появляются в drop directory целевой машины.
2. Папки копируются рекурсивно; структура вложенности сохраняется.
3. Передача по тому же TLS-соединению (порт 24800); отдельный канал не нужен.
4. Clipboard синхронизируется **pull-on-demand**: grab анонсирует владение, содержимое передаётся только при явной вставке. Не передаём большие файлы впустую.
5. Drop directory настраивается в конфиге; по умолчанию — `<user_download_dir>/InputLeap/` (через `directories::UserDirs`).
6. Для передач > 10 МБ IPC шлёт в GUI события прогресса (обоим: отправителю и получателю) каждые 5% или 1 с, что раньше.
7. Если получатель не анонсировал `Capability::FileClipboard` в `Hello` — сервер не анонсирует файловый clipboard при `ClipboardGrab`, fallback на текстовый.
8. Отмена передачи через GUI или обрыв соединения — корректная очистка незавершённых временных файлов (через `Drop` guard + `.part`-файлы).
9. Максимальный объём одной передачи ограничен настраиваемым лимитом (по умолчанию 2 ГиБ).
10. Симметрия: клиент → сервер работает идентично (Ctrl+C на secondary, Ctrl+V на primary); drop directory настраивается в конфиге каждой машины.

## User / system flow

```
[Машина A]                                  [Машина B]

1. Пользователь нажимает Ctrl+C на файлах
   PlatformScreen::read_file_clipboard() →
       Vec<PathBuf> (или None, если clipboard не файловый)
   → локальный state: FileClipboardSlot { paths, seq }

2. Курсор переходит на B (screen switch)
   → Message::ClipboardGrab { id: File, seq }

3. Пользователь нажимает Ctrl+V на B
   → Message::ClipboardRequest { id: File, seq }

4. A получает ClipboardRequest:
   → spawn TransferSender task:
       Message::FileTransferStart { transfer_id, manifest }
       Message::FileChunk { transfer_id, data }   (много раз)
       Message::FileTransferEnd { transfer_id }
   → IPC: ProgressEvent { direction: Sending, percent, ... }

5. B получает стрим:
   → spawn TransferReceiver task
   → пишет файлы в drop_directory/<transfer_id>/ как .part
   → по FileTransferEnd — атомарный rename .part → final name
   → инжект в OS clipboard списка URI принятых файлов
   → IPC: ProgressEvent { direction: Receiving, ... }

6. По завершении: IPC → GUI: TransferComplete { count, bytes }
```

## Technical approach

### Новые типы (`common` crate)

```rust
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    Text,
    Html,
    Bitmap,
    Files,       // <-- новый
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileManifestEntry {
    pub rel_path: PathBuf,   // относительный путь от корня копирования
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileManifest {
    pub entries: Vec<FileManifestEntry>,
    pub total_bytes: u64,
}

pub type TransferId = u64;   // уникальный per connection
```

### Новые сообщения протокола (`protocol` crate)

Расширяем `enum Message` в `specs/rust-rewrite.md`:

```rust
// В enum Message:
ClipboardRequest { id: ClipboardId, seq: u32 },
FileTransferStart {
    transfer_id: TransferId,
    clipboard_seq: u32,
    manifest: FileManifest,
},
FileChunk {
    transfer_id: TransferId,
    entry_index: u32,     // индекс файла в manifest
    data: Bytes,
},
FileTransferEnd { transfer_id: TransferId },
FileTransferCancel {
    transfer_id: TransferId,
    reason: TransferCancelReason,
},

#[derive(Debug, Serialize, Deserialize)]
pub enum TransferCancelReason {
    UserCancelled,
    DiskFull,
    SizeMismatch,       // файл на диске отправителя изменился
    PeerError,
    PathTraversal,
}
```

**Почему `entry_index` в `FileChunk` вместо неявной упорядоченности по манифесту:** защита от любой рассинхронизации состояния; получатель может писать в правильный файл даже если теряется состояние «какой файл сейчас пишем» (например, на reconnect в будущем).

**Размер chunk:** 64 КиБ по умолчанию (`FILE_CHUNK_BYTES: usize = 65536`). Подбирается эмпирически в M9; lower bound — MTU-friendly, upper bound — `max_frame_length` (16 МиБ).

### Capability-negotiation

В `HelloPayload::capabilities` клиент и сервер анонсируют `Capability::FileClipboard`. Сервер при `ClipboardGrab { id: File }` проверяет, что получатель анонсировал capability. Если нет — не шлёт grab для файлов; текстовый clipboard продолжает работать.

### Platform clipboard (`platform/*` crates)

Добавить в `PlatformScreen` методы:

```rust
fn read_file_clipboard(&self)
    -> impl Future<Output = Result<Option<Vec<PathBuf>>>> + Send;

fn write_file_clipboard(&self, paths: &[PathBuf])
    -> impl Future<Output = Result<()>> + Send;
```

Реализации:
- `x11/`: чтение/запись `text/uri-list` через `x11rb` selection API (`XFixes` для уведомлений).
- `macos/`: `NSPasteboard` с `NSFilenamesPboardType` через `objc2` crate.
- `windows/`: `CF_HDROP` через `windows-rs` (`DragQueryFileW`, `GlobalAlloc`).
- `ei/`: `text/uri-list` через portal'овский `ext-data-control` (или `wl_data_device` если доступен).

### Server / Client логика

**Отправитель (`server` или `client` — симметрично):**
- `TransferSender` — отдельная `tokio::task` на каждую передачу.
- Обходит дерево через `tokio::fs::read_dir` (recursive, `async_recursion` крейт или stack-based).
- Строит `FileManifest`, проверяет `total_bytes <= max_transfer_bytes`.
- Читает файлы chunked через `tokio::fs::File::read_buf`.
- Шлёт `FileChunk`, ожидая backpressure из `net` крейта (`tokio::sync::mpsc` с bounded channel).

**Получатель:**
- `TransferReceiver` task.
- Создаёт `<drop_dir>/<transfer_id>/` как staging directory.
- Пишет каждый файл как `<rel_path>.part`.
- По `FileTransferEnd` — атомарный `tokio::fs::rename` в `<drop_dir>/<manifest_root_name>/`.
- При конфликте имён: `name_1`, `name_2`, ... (ищет свободное имя).
- Инжектит URI-list в OS clipboard через `write_file_clipboard`.

**Cancellation:**
- Любая сторона шлёт `FileTransferCancel`.
- `Drop` guard на staging directory удаляет все `.part`-файлы при падении task.

### Безопасность

- **Path traversal:** каждая `rel_path` в manifest проходит валидацию:
  - Не содержит `..`-компонентов
  - Не абсолютный путь
  - Не начинается с `/` или drive-letter
  - После `canonicalize` остаётся внутри staging dir
- При нарушении — `FileTransferCancel { reason: PathTraversal }` + `tracing::error!` как security event.
- **Symlinks:** отправитель skipping symlinks с `tracing::warn!` (v1); в v2 — опция follow.
- **Максимальный объём:** из конфига, default 2 ГиБ. При превышении — reject на этапе manifest, не начинаем передачу.
- **Права IPC/socket:** см. основной спек.

### Конфиг

```toml
[file_transfer]
enabled = true
drop_directory = "~/Downloads/InputLeap"       # расширяется через directories
max_transfer_bytes = 2_147_483_648              # 2 GiB
chunk_bytes = 65536
follow_symlinks = false                         # reserved for future
```

Путь `drop_directory` резолвится через `shellexpand` + `directories::UserDirs::download_dir()` как fallback.

### IPC события

```rust
// notification из daemon в GUI
pub enum IpcNotification {
    // ...
    TransferStarted {
        transfer_id: TransferId,
        direction: TransferDirection,
        peer_name: String,
        total_bytes: u64,
        file_count: u32,
    },
    TransferProgress {
        transfer_id: TransferId,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    TransferCompleted {
        transfer_id: TransferId,
        bytes_transferred: u64,
    },
    TransferCancelled {
        transfer_id: TransferId,
        reason: TransferCancelReason,
    },
}

pub enum TransferDirection { Sending, Receiving }
```

## Edge cases & error handling

- **Файл изменился во время чтения:** отправитель сравнивает реально прочитанные байты с `size` из manifest; несовпадение → `FileTransferCancel { reason: SizeMismatch }`.
- **Нет места на диске получателя:** `tokio::io::Error { kind: StorageFull }` → `FileTransferCancel { reason: DiskFull }`, удалить `.part`-файлы.
- **Конфликт имён в drop_dir:** автосуффикс `_1`, `_2`, ... без диалога (v1).
- **Path traversal:** немедленный `FileTransferCancel { reason: PathTraversal }`, drop соединения, `tracing::error!` с peer name и offending path.
- **Обрыв соединения в середине:** `Drop` guard на `TransferReceiver` удаляет staging directory целиком.
- **Clipboard захвачен другим процессом (X11):** `read_file_clipboard` возвращает `Ok(None)`, silent fallback на текстовый clipboard.
- **Старый peer без `Capability::FileClipboard`:** не анонсируем файловый grab; текстовый clipboard работает как обычно.
- **Пустой manifest (копирование пустой папки):** легальный случай, создаём пустую папку в drop_dir.
- **Manifest с нулевым `total_bytes` но ненулевым `entries.len()`:** легально (все файлы — пустые). Обрабатываем штатно.
- **Обратное давление:** если получатель отстаёт — bounded `mpsc` в `net` крейте создаёт backpressure на `TransferSender::read_file`, он блокируется на `await`.
- **Несколько одновременных передач:** поддерживаются (`transfer_id` уникальный); каждая — своя task и staging dir.

## Open questions

1. **Прогресс отправителю: через IPC того же демона или через обратное сообщение в протоколе?**
   Рекомендую через локальный IPC — отправитель знает свой прогресс сам, не нужен round-trip по сети.
2. **Permissions/xattr на приёме:** v1 игнорирует (файлы создаются с umask текущего пользователя). Добавлять в v2 только по явному запросу — кросс-платформенная семантика сложна.
3. **Compression for text-heavy trees:** zstd-поток поверх `FileChunk`? Отложить до v2, замерить реальные use-cases.
4. **UI прерывания:** GUI-кнопка «отменить передачу» должна слать IPC-request `cancel_transfer { transfer_id }` → daemon шлёт `FileTransferCancel`. Проектируется вместе с IPC в M5, а не M9.
