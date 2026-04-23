# M1 — `protocol` crate: CBOR-сообщения v1, codec, тесты

## Цель

Реализовать полную схему сетевых сообщений Hop v1 на serde+CBOR с length-delimited фреймингом. По завершении M1 любой крейт может импортировать `hop_protocol::{Message, Codec, ProtocolError}` и работать с серилизованными сообщениями поверх произвольного `AsyncRead`/`AsyncWrite`, даже без настоящей сети.

## Предпосылки

- [M0](M0-skeleton.md) — скелет воркспейса

## Scope

**In scope:**
- Типы сообщений `Message` (enum) со всеми вариантами, перечисленными в основном спеке
- Вспомогательные типы: `HelloPayload`, `DeviceInfoPayload`, `Capability`, `DisconnectReason`, `KeyId`, `ButtonId`, `ClipboardId`, `ClipboardFormat`, `ModifierMask`
- CBOR сериализация/десериализация через `ciborium`
- Фрейминг: `tokio_util::codec::LengthDelimitedCodec`, max frame 16 МиБ
- `Encoder`/`Decoder` типы, скомбинированные через `FramedWrite`/`FramedRead`
- `ProtocolError` через `thiserror`
- Property tests (`proptest`): round-trip всех вариантов `Message`
- Golden snapshot tests (`insta`): hex-дампы canonical-байтов для каждого варианта (документируют wire-формат)
- Документация модуля на уровне `//!` с примером encode/decode

**Out of scope:**
- File clipboard сообщения (M9)
- Сеть (M2) — `protocol` работает поверх любого `AsyncRead`/`AsyncWrite`
- TLS (M2)
- Handshake state machine (M2)

## Задачи

### Типы сообщений

- [ ] `crates/common/src/ids.rs`:
  - `KeyId(u32)`, `ButtonId(u8)`, `ClipboardId(u8)` как newtype-обёртки
  - `ModifierMask(u32)` как bitflags через `bitflags` crate
  - `ClipboardFormat` enum
  - Derive `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`
- [ ] `crates/protocol/src/message.rs`:
  - `enum Message` с `#[serde(tag = "type")]` и всеми вариантами из основного спека
  - `HelloPayload { protocol_version, display_name, capabilities }`
  - `DeviceInfoPayload { width, height, scale_factor, ... }`
  - `enum Capability` с `#[serde(rename_all = "snake_case")]` и `#[serde(other)] Unknown` для forward-compat
  - `enum DisconnectReason { ProtocolVersionMismatch, KeepAliveTimeout, UnknownPeer, MalformedMessage, FrameTooLarge, UserInitiated, InternalError, ... }`
- [ ] `crates/protocol/src/version.rs`:
  - `pub const PROTOCOL_VERSION: u16 = 1;`
  - `pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;`

### Codec

- [ ] `crates/protocol/src/codec.rs`:
  - `struct MessageCodec` с inner `LengthDelimitedCodec`
  - `impl Encoder<Message> for MessageCodec` — ciborium в `BytesMut`
  - `impl Decoder for MessageCodec { type Item = Message; }` — чтение фрейма, затем ciborium из среза
  - Ошибки маппятся в `ProtocolError`
- [ ] Helper: `pub fn framed<T: AsyncRead + AsyncWrite>(io: T) -> Framed<T, MessageCodec>`

### Error handling

- [ ] `crates/protocol/src/error.rs`:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum ProtocolError {
      #[error("io error: {0}")]
      Io(#[from] std::io::Error),
      #[error("CBOR decode failed: {0}")]
      Decode(#[from] ciborium::de::Error<std::io::Error>),
      #[error("CBOR encode failed: {0}")]
      Encode(#[from] ciborium::ser::Error<std::io::Error>),
      #[error("frame exceeds max size: {size} > {MAX_FRAME_BYTES}")]
      FrameTooLarge { size: usize },
  }
  ```

### Тесты

- [ ] `crates/protocol/tests/roundtrip.rs`:
  - `proptest!` для каждого варианта `Message` — генерируется случайная инстанция, сериализуется, десериализуется, сравнивается (`assert_eq!`)
  - `Arbitrary` реализации через `proptest-derive` где возможно, вручную — для типов с инвариантами (например, `ModifierMask` — только валидные биты)
- [ ] `crates/protocol/tests/snapshots.rs`:
  - Для каждого варианта `Message` — canonical instance → serialize → `insta::assert_snapshot!(hex_dump)`
  - Snapshots коммитятся в репо — документируют wire-формат и ловят несанкционированные изменения схемы
- [ ] `crates/protocol/tests/framing.rs`:
  - Два `Message` пишутся в `Vec<u8>` через `FramedWrite`, читаются через `FramedRead` — получаются те же значения
  - Обрезанный фрейм → decoder возвращает `Ok(None)` (need more bytes), не ошибку
  - Фрейм с длиной > `MAX_FRAME_BYTES` → `ProtocolError::FrameTooLarge`
  - Корректная длина + битый CBOR внутри → `ProtocolError::Decode`
- [ ] Fuzz target (опционально, если время): `cargo-fuzz` на decoder — любые байты не должны паниковать

### Документация

- [ ] `crates/protocol/src/lib.rs` — `//!` module-level docs с примером:
  ```rust
  //! # Example
  //! ```no_run
  //! # use tokio::net::TcpStream;
  //! # use hop_protocol::{framed, Message, HelloPayload, Capability};
  //! # async fn demo(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
  //! use futures::{SinkExt, StreamExt};
  //! let mut conn = framed(stream);
  //! conn.send(Message::Hello(HelloPayload {
  //!     protocol_version: 1,
  //!     display_name: "laptop".into(),
  //!     capabilities: vec![Capability::UnicodeClipboard],
  //! })).await?;
  //! let reply = conn.next().await.transpose()?;
  //! # Ok(()) }
  //! ```
- [ ] `docs/wire-format.md` (под `specs/`? — решить при реализации) — человекочитаемое описание wire-формата для будущих реализаций в других языках

## Acceptance criteria

- [ ] Все варианты `Message` имеют property test round-trip — 0 fail на 10k итераций
- [ ] Snapshot-тесты: по одному canonical instance на вариант, зафиксированы в `crates/protocol/tests/snapshots/`
- [ ] `cargo bench` (опционально, но желательно) показывает encode/decode `KeepAlive` < 1 мкс и `MouseMove` < 5 мкс на современном CPU
- [ ] Публичное API крейта документировано: `cargo doc --no-deps` не выдаёт missing-docs warnings
- [ ] CI проходит; crate docs попадают в будущий docs.rs без warnings
- [ ] `cargo deny check` green

## Тесты

В дополнение к списку выше:
- [ ] Каждый `Capability` вариант сериализуется в известный snake_case string (не случайный автодетект от serde)
- [ ] Forward-compat: сообщение с неизвестным `Capability` string внутри `Hello` — десериализуется в `Capability::Unknown`, остальной `Hello` парсится штатно
- [ ] `DisconnectReason` с неизвестным вариантом — graceful fallback (например, `DisconnectReason::Unknown(String)`? — или `Disconnect { reason: Unknown }`; решить при реализации)

## Риски / open questions

1. **CBOR map encoding vs array encoding:** `ciborium` по умолчанию кодирует struct как map (имена полей в wire-формате). Плюс — forward-compat через `#[serde(skip)]`. Минус — больше байт на wire (для `MouseMove` это заметно: 2 поля × 3 байта ключ + 2 байта значение vs 4 байта `[x, y]` как array). Решение для M1: **оставить map-encoding** (читаемость wire-формата важнее нескольких байт, при 1000 событий/сек overhead < 50 KB/s). Если замеры в M3 покажут проблему — перейти на manual `serde::Serializer` с array encoding для hot-path сообщений.
2. **Endianness length-prefix:** `LengthDelimitedCodec` default — big-endian 4 байта. Подтвердить и зафиксировать в `docs/wire-format.md`.
3. **`#[serde(other)]` для `Capability`:** требует `#[serde(untagged)]` или `tag`-free enum? — уточнить в реализации; может потребовать `#[serde(other)] Unknown(String)` или кастомного Deserialize.
4. **Protobuf alternative?** — отказано сознательно: CBOR не требует кодогенерации и IDL, derive-сериализация даёт тот же уровень кросс-языковости. Не пересматривать без веской причины.
5. **Nested `Bytes` в `ClipboardData`:** `bytes::Bytes` + serde — нужна feature `serde` у `bytes`; проверить в M0 что включено.

## Готовность к M2

После M1 у нас есть:
- Типы и codec для любых дальнейших слоёв
- Увереность в wire-формате через snapshot-тесты
- Fuzz-safe decoder

Это минимум, необходимый для M2 (handshake over TLS).
