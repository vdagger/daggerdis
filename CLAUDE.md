# Albus Audio — CLAUDE.md

Serverless P2P mesh voice radio (walkie-talkie) in pure Rust. No central media
servers — every client is a full mesh node. End-to-end-encrypted voice over
**Iroh (QUIC)**, with a native **eframe/egui** GUI.

## Project facts

- Crate: `albus` v0.1.0, edition 2021. Binary app; `src/lib.rs` exists only to share
  modules with the `main.rs` bin target.
- Repo: https://github.com/vdagger/daggerdis
- Primary platform: **Windows 10/11** (admin manifest). Also builds on Linux/WSL2 and
  macOS (GitHub Actions "Build macOS Universal").
- Stack: `eframe`/`egui` 0.28, `cpal` 0.15, `rusty-opus` 1.0 (Opus 64 kbps),
  `iroh` 1.4 (QUIC), `tokio` (full), `serde` + `serde_json`, `bytes`,
  `raw-window-handle` 0.6 (HWND для тёмного заголовка).
  Windows-only: `windows` 0.62 (DXGI Desktop Duplication, D3D11, Media Foundation,
  `Win32_Graphics_Dwm` для `DwmSetWindowAttribute`); other platforms build without
  it (`src/video` is gated).

## Build & run

### Windows

```powershell
cargo build --release
.\target\release\albus.exe
```

### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install -y pkg-config libasound2-dev libx11-dev libxcursor-dev \
  libxrandr-dev libxi-dev libgl1-mesa-dev libxkbcommon-dev libssl-dev libclang-dev
cargo build --release && ./target/release/albus
```

### macOS

Built via GitHub Actions (`workflow_dispatch` → "Build macOS Universal"), or locally
with `cargo build --release`.

The app requests **Administrator** (UAC manifest injected by `build.rs`/`winres`) so it
can register firewall rules and install a global PTT keyboard hook. Launching from a
normal terminal triggers a UAC prompt.

## Architecture (startup flow, `src/main.rs`)

1. `firewall::setup_firewall_rules()` — auto-add Windows inbound UDP rule (covers iroh QUIC).
2. `identity::Identity::load_or_create()` — load the pinned iroh `SecretKey` from
   `%APPDATA%\Albus\secret.key` (or generate + save). `my_id: u64` = FNV-1a hash of the
   NodeID (stable routing alias, not a key).
3. `IrohP2pManager::new(secret_key)` → publishes a stable `NodeID`. On failure the app
   still runs GUI-only and shows an error banner (`RadioApp.network_error`).
4. `config::AppConfig::load()` + `friends::FriendsManager::load()` (friends.json).
5. `RoomManager`, `AudioMixer`, `UniversalAudioCapture`, `MigrationManager`.
   Mixer levels/channels/profiles, PTT key and hands-free come from config; mic/playback
   streams open the saved devices by name (fallback: system default). Per-peer volumes
   restored. If `input2_device` names a different card, two input streams open
   (`[true,false]` and `[false,true]` slots); otherwise one stream fills both slots.
6. Two mpsc channels:
   - `packet_tx/rx: Vec<u8>` — outbound encoded voice.
   - `playback_tx/rx: (u64, u32, Vec<u8>)` — inbound `(sender_id, seq, opus)`.
7. Spawn background tasks:
   - `iroh_service::spawn_iroh_receiver` — Iroh listener + full-mesh + relay + friends + chat + calls + presence.
   - `broadcast::spawn_audio_broadcast` — fan-out voice over Iroh.
   - `presence::spawn_ping_loop` — пинг друзей раз в 4 с (`PresencePing`/`Pong`), только если iroh поднялся.
   - `MigrationManager::start_heartbeat_loop` — 1 s IrohSync / host failover.
8. On Windows: `video_link::VideoLink::spawn(p2p, rt.handle(), video_directory,
   is_host, room)` — the video transport (its own ALPN) sharing the
   `VideoDirectory` that carries the live-broadcast announcements (IrohSync
   `video_gen`). It needs `is_host` and `room` because the room host accepts a
   publisher's feed and relays it, and admissions must stop after «Выйти из
   эфира». It must be spawned with the runtime `Handle`, not bare `tokio::spawn` —
   at this point `main` has not entered the runtime yet. The GUI receives link +
   directory via `RadioApp::attach_video(link, directory)`.
9. `eframe::run_native("Albus Audio", ...)` → `gui::RadioApp`.

Transport is Iroh-only. The old raw-UDP mesh (`receiver.rs`, `manager.rs`, `stun.rs`,
`upnp.rs`, `ip_detector.rs`) was removed — there is no plain-UDP fallback.

## File map

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | Dependencies (tokio, iroh, cpal, rusty-opus, egui/eframe, serde, serde_json, bytes) |
| `build.rs` | `winres` UAC manifest (`requireAdministrator`) on Windows |
| `src/lib.rs` | Module re-exports + `pub use types::Peer` |
| `src/main.rs` | Entry point: identity, iroh init, wire everything, spawn tasks, run eframe |
| `src/types.rs` | `Peer { id, address, is_host, last_heartbeat }` — `address` holds the bare NodeID |
| `src/identity.rs` | Pinned iroh `SecretKey` (`secret.key`), `my_id` = FNV-1a hash of NodeID, reset |
| `src/friends.rs` | `FriendsManager`: `friends.json` (node_id/name/date), incoming/outgoing requests |
| `src/presence.rs` | `PresenceTracker`: кто из друзей недавно подавал признаки жизни (TTL 12 с) + фоновый ping-цикл раз в 4 с |
| `src/chat.rs` | `ChatLog`: session-only DM history in memory (per-NodeID threads, unread counts) |
| `src/calls.rs` | `CallManager`: состояние звонка (Idle/Outgoing/Incoming/Active), переходы под одним замком, проверка `call_id` в Accept/Decline/Cancel/End, приглашения с переотправкой, таймауты, «занято», блокировка нодоступного собеседника в комнате, тон звонка |
| `src/room.rs` | `RoomManager`: `Mutex<Vec<Peer>>`, peer upsert/remove, next-host election, флаг `left` («Выйти из эфира») |
| `src/migration.rs` | `MigrationManager`: 1 s IrohSync broadcast, host failover |
| `src/audio.rs` | `UniversalAudioCapture`: CPAL capture/playback, линейный ресэмплер, per-peer mix bus. Вход пишет сэмплы своего устройства в слоты `txbus`, вывод кладёт в `echo_ref` опору AEC; Opus-энкодера и DSP в колбэке больше нет |
| `src/txbus.rs` | `InputBus` (кольца на два входа, эпохи, watchdog) + поток `albus-tx`: сборка кадра 20 мс (960 сэмплов), эффекты входов по тумблерам (эхо-подавление AEC3, шумодав RNNoise, затвор и AGC независимо), лимитер передачи, монитор, Opus 64/96/128 кбит/с |
| `src/mixer.rs` | `AudioMixer` (per-peer dB gain, master gain, mute, VU, monitor своего тракта, кольцо опоры `echo_ref`, тумблеры `input1_denoise`/`input1_gate`/`input1_agc`/`input1_aec`/`input2_denoise`/`input2_gate`/`input2_agc`/`input2_aec`/`tx_limiter_enabled`/`rx_limiter_enabled` — все по умолчанию выкл., битрейт `opus_bitrate`), `SlotProfile` (пресет тумблеров), `TransmitAgc`, `Limiter` (−1 dBFS, на микс) |
| `src/devices.rs` | `DeviceSelector`: enumerate input/output devices, find by name |
| `src/config.rs` | `AppConfig`: `%APPDATA%\Albus\config.ini` (devices включая `input2_device`, PTT, mixer, эффекты `input1_denoise`/`input1_gate`/`input1_agc`/`input1_aec`/`input2_denoise`/`input2_gate`/`input2_agc`/`input2_aec`/`tx_limiter`, nick, per-peer volume, `video_fps`, `video_buffer_ms`, `rx_limiter`; старые `mic_processing` и `input*_profile` читаются для совместимости) |
| `src/keybind.rs` | `PttBind`: global Win32 `GetAsyncKeyState` hook thread |
| `src/firewall.rs` | `setup_firewall_rules()`: Windows firewall inbound UDP rule for the current exe — идемпотентно (лишние одноимённые записи чистятся) |
| `src/menu.rs` | `handle_command()`: console `/mute`, `/vol`, `/mic`, `/devices` (dormant — see below) |
| `src/network/mod.rs` | Re-exports `NetworkMessage`; declares `broadcast`, `iroh_service`, `message`, `p2p_iroh`, `video_directory`, `video_link` |
| `src/network/p2p_iroh.rs` | `IrohP2pManager`: QUIC datagrams + uni-стримы (`send_bytes` с переподключением, `spawn_stream_receiver`), connect by NodeID, `broadcast_bytes`, `disconnect_peer`/`disconnect_all` |
| `src/network/iroh_service.rs` | `spawn_iroh_receiver`: Iroh listener, sender_id↔NodeID check, full-mesh, relay, friends, chat, звонки (гейт по `CallManager::is_blocked`) |
| `src/network/broadcast.rs` | `spawn_audio_broadcast`: parallel voice fan-out over Iroh |
| `src/network/message.rs` | `NetworkMessage` enum + hand-rolled binary (de)serialization |
| `src/gui/mod.rs` | `RadioApp`: eframe window, "Cyber Slate" theme, 60 FPS, network-error banner; console layout bottom-up, rail as one card, settings/chat overlays |
| `src/gui/theme.rs` | Tokens (PAGE/SURFACE/INNER/INSET, BRAND, LIVE, STROKE_SOFT), `draw_icon`, `avatar`, `primary_button`, `chip_button`, radial fills/rings, level bars |
| `src/widgets/hero.rs` | Диск рации (шкала, дуга уровня, кольцо с точкой), заголовок эфира, полоса «линия» |
| `src/gui/stream_control.rs` | Hot-swap audio devices at runtime |
| `src/widgets/audio_settings.rs` | Settings grid: VU meter, sliders, device lists, тумблер прослушивания тракта |
| `src/widgets/info_panel.rs` | Header: logo, user ID, copyable NodeID, settings entry |
| `src/widgets/identity.rs` | "Мой NodeID" block: full ID + copy button/click-to-copy |
| `src/widgets/friends_panel.rs` | Friends list, incoming requests, add-friend, call (→ `CallManager::start_outgoing`), remove, chat button + unread |
| `src/widgets/chat_panel.rs` | DM chat: message bubbles, input, send over iroh datagrams |
| `src/widgets/ptt_button.rs` | Hands-free toggle, on-screen PTT, TX indicator |
| `src/video/mod.rs` | OS-independent: `VideoPreset`/`VideoTarget` (bitrate from pixel count), `StreamInfo`, `EncodedUnit` |
| `src/video/win_dxgi.rs` | DXGI Desktop Duplication capture; `next_nv12()` returns one frame as NV12 or `None` |
| `src/video/win_d3d.rs` | D3D11 device + `ID3D11VideoProcessor` BGRA↔NV12 conversion/scaling, staging readback |
| `src/video/win_mf.rs` | Async hardware H.264 MFT encoder (NVENC/AMF/QSV), CBR/GOP via `ICodecAPI` |
| `src/video/win_decode.rs` | H.264 decoder MFT — both async and sync paths; NV12 repack 1088→1080 |
| `src/video/publisher.rs` | `video-publisher` thread: capture at the chosen fps (30/60) → NV12 → encode → mpsc of `EncodedUnit`; `encode_fps` counter for diagnostics |
| `src/network/video_directory.rs` | `VideoDirectory`: номер своей трансляции (`my_gen`) + анонсы соседей из IrohSync (`video_gen`), TTL 5 с; общая для миграции, сервиса приёма и видео |
| `src/network/video_link.rs` | `discordrf-video/1`: контроль `kind=2/3/4/5` (подписка, ответ, feed издателя, релей-подписка) в первом bi-стриме, окно недавних кадров и relay под одним замком (180 / 64 МиБ, отдача с последнего ключевого), релей хоста без ре-энкода, маршрутизация зрителя, очередь зрителя со сдвигом показа на буфер |
| `src/gui/video_session.rs` | `Renderer` (decode → BGRA → egui texture), `VideoSession` (own stream), `RemoteView` (peer stream) |
| `tools/video_probe/` | Separate probe crate: `--capture`, `--gpu`, `--loopback` headless checks |
| `src/widgets/room_panel.rs` | Секции рейла: вход по NodeID, «Мой NodeID» + копирование, участники плоскими строками, громкость собеседника |
| `assets/icon.png`, `assets/logo.png` | Embedded via `include_bytes!` (monolithic binary) |
| `.github/workflows/macos.yml` | macOS Universal 2 build |

## Wire protocol (`src/network/message.rs`)

Hand-rolled binary framing — **not** serde/bincode on the wire. First byte is a tag.
Integers are **little-endian**; text fields are `len u16 LE` + UTF-8 bytes.

| Tag | Message | Layout (after tag) |
|-----|---------|--------------------|
| 0 | `Heartbeat` | `sender_id` u64 LE, `is_host` u8 |
| 3 | `AudioFrame` | `sender_id` u64 LE, `seq` u32 LE, `opus_data` (rest of payload) |
| 4 | `HostMigration` | `sender_id` u64 LE, `new_host_id` u64 LE (принимается только если `sender_id == new_host_id` и совпадает с NodeID соединения) |
| 5 | `IrohSync` | `sender_id` u64 LE, `is_host` u8, `hops` u8 (0 — от самого узла, 1 — переслан хостом, дальше не релеится), `len` u16 LE, `node_id` UTF-8, `video_gen` u32 LE — номер трансляции (0 = не в эфире); поле в конце, старые парсеры его игнорируют, новый без него читает 0 |
| 6 | `FriendRequest` | `sender_id` u64 LE, `node_id` str, `nick` str |
| 7 | `FriendAccept` | `sender_id` u64 LE, `node_id` str, `nick` str |
| 8 | `FriendRemove` | `sender_id` u64 LE, `node_id` str |
| 9 | `ChatMessage` | `sender_id` u64 LE, `text` str |
| 10 | `CallInvite` | `sender_id` u64 LE, `call_id` u32 LE, `nick` str |
| 11 | `CallAccept` | `sender_id` u64 LE, `call_id` u32 LE |
| 12 | `CallDecline` | `sender_id` u64 LE, `call_id` u32 LE, `reason` u8 (0 отклонён / 1 занято / 2 таймаут) |
| 13 | `CallCancel` | `sender_id` u64 LE, `call_id` u32 LE |
| 14 | `CallEnd` | `sender_id` u64 LE, `call_id` u32 LE |
| 15 | `PresencePing` | `sender_id` u64 LE — «ты в сети?»; пинг живёт вне комнаты |
| 16 | `PresencePong` | `sender_id` u64 LE — ответ на пинг |

Tags 1 (`JoinRequest`) and 2 (`PeerListUpdate`) were removed with the raw-UDP mesh.

### Video wire format (`src/network/video_link.rs`)

Video uses a **separate iroh connection** with ALPN `discordrf-video/1`: QUIC
congestion control is per-connection, so video cannot starve voice datagrams.
Each frame is its own **unidirectional stream** — a lost packet delays one frame,
not the whole GOP. Little-endian 28-byte header, then optional SPS/PPS and the
Annex-B H.264 payload:

| Offset | Type | Field |
|--------|------|-------|
| 0 | u8 | kind = 1 |
| 1 | u8 | flags: 1 = keyframe, 2 = has SPS/PPS |
| 2 | u16 | seq_len |
| 4 | u32 | width |
| 8 | u32 | height |
| 12 | u32 | fps |
| 16 | i64 | pts (100 ns) |
| 24 | u32 | payload length |
| 28 | bytes | SPS/PPS (if flag 2), then H.264 |

**Control protocol.** The client opens the connection — a viewer, or a publisher
sending its feed to the host. The first bidirectional stream is the control
channel; the first byte says what it is (read limit 256 bytes):

| kind | direction | layout |
|------|-----------|--------|
| 2 | viewer → publisher | `gen u32` — direct subscription |
| 3 | answer | `status u8` (0 = accepted, 1 = refused), `gen u32` |
| 4 | publisher → host | `gen u32` — feed |
| 5 | viewer → host | `gen u32`, `len u16`, publisher NodeID (64 hex) — relay subscription |

On acceptance the node starts the right task on that connection (sender for a
viewer, `read_feed` for a feed); on refusal it closes the connection. The client
closes its control send side right after the request, so the server detects its
departure with `Connection::closed()`, not with EOF. An answer that takes longer
than 3 s is retried no sooner than 2 s later. The publisher NodeID in kind 5 is
mandatory: `gen` is taken from the clock, two publishers collide in the same
second, and `decide_control` compares the pair (publisher, gen) against the
current relay.

**Topology is a star.** The publisher sends **one** stream to the room host
(kind 4); the host forwards the same compressed bytes to every viewer (kind 5)
and into its own preview window — no re-encode. Publisher upload is 1 × bitrate,
host upload is N × bitrate; the extra hop is absorbed by the viewer's buffer.
A host that broadcasts itself takes no relay and is not used as one: viewers
open a direct subscription (kind 2) to it. There is no fallback to the direct
path — `choose_view_route` either finds a route or leaves the viewer empty, as
the builds must match by stamp anyway. The host admits a feed only when
`is_host && !room.is_left() && my_gen == 0 && directory.is_live(from, gen)`; a
new feed replaces the old one (its task receives `stop`, `Recent` is cleared),
and the old read task removes the relay only when `relay.id` is still its own.

The publisher announces `my_gen` in `IrohSync` (tag 5, `video_gen`, sent once a
second); `VideoDirectory` keeps the announcements of neighbours with a 5 s TTL
and `live(now)` returns `(node_id, gen)` pairs. Every node picks the first live
announcement not its own, so all converge on one stream: `choose_view_route`
gives `Relay` through the host (when the host exists, is not the publisher and
is not broadcasting), `FeedWatch` to the host itself, and `Direct` otherwise;
`choose_feed_target` makes a publisher feed the host only when no other live
announcement exists. So a peer who joins the room mid-stream subscribes within
about a second without any manual recipient picker.

`RoomManager::host_peer()` returns the peer with the lowest `id` among
`is_host` — the same rule `migration.rs` uses to elect a host. Self is not in
`peers`, so `None` means "I am the host, or there is no host yet".

**Late joiners start from a keyframe.** `publish` always keeps a ring of the
last 180 frames / 64 MiB (`Recent`), keyed by an in-memory `seq`, even when
nobody is subscribed. `handoff()` creates the broadcast receiver and snapshots
the ring from the last keyframe **under the same mutex `publish` uses**, so a
new subscriber gets a gap-free chain; P-frames before the first keyframe are
skipped (if the ring has no keyframe, the task waits for the next IDR). The
broadcast capacity is 240 frames; after `Lagged` the task takes a fresh handoff
instead of continuing from a P-frame. The wire frame format is unchanged.

The ring and the relay live under **one mutex** (`Window { recent, relay }`):
`publish_own` re-checks `directory.my_gen()` under the lock, `publish_relay`
requires `my_gen == 0` and its own relay id, so a frame from a stopped stream
can never land in a new one and a subscription never sees two streams mixed.
`remote_epoch` is bumped on every queue reset — the GUI drops `RemoteView` even
when the source name did not change.

Stopping a broadcast calls `VideoLink::end_stream()` (gen 0, `stop_viewers()`,
ring cleared, outgoing feed stopped, relay and viewer queue dropped); the GUI
does it in the same click that drops `self.video` and in
`leave_voice`. Each viewer connection lives inside a send task that selects on a
`watch` stop signal; when it fires, the task ends and with it the QUIC
connection — without this every stop/start cycle left the old connections alive
and added a fresh one per peer. On the receiving side a finished stream drops
that sender's queued frames and waits for a keyframe.

## Key implementation details & gotchas

- **GUI-оболочка (макет `albus-mockup/slide-desktop.png`):** тёмное окно без
  разделителей между колонками; консоль слева, рейл справа. Рейл — **одна
  карточка** на всю высоту: секции (`rail_section`, отступы 14/11) разделены
  линиями во всю ширину (`rail_divider`), подвал «Сквозное шифрование · v0.1»
  прижат к низу внутри карточки, список в `ScrollArea` с `max_height = available
  − 28`.
- **`with_layout(right_to_left)` в вертикальном UI съедает всю оставшуюся высоту
  ячейки.** Из-за этого карточка «Комната» когда-то растянулась и вытолкнула
  участников из вида. Строку с правым элементом всегда оборачивать в
  `ui.horizontal`.
- **Геометрия консоли считается снизу вверх** (`draw_console`): сначала полоса
  «линия» (56), затем PTT-пилюля (54), затем подвал (26), остаток — эфир;
  ширина диска = `width * 0.46` в диапазоне 230..330. Так PTT и подвал всегда
  прижаты к низу при любом размере окна.
- **Тёмный системный заголовок окна:** `RadioApp::apply_dark_titlebar` берёт
  `eframe::Frame::window_handle()` (raw-window-handle 0.6), достаёт `HWND` и
  вызывает `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE)` один раз
  (`self.title_dark`). Без этого белая полоса ломает тёмный макет. Нужны
  зависимости `raw-window-handle` и feature `Win32_Graphics_Dwm`.
- **Подсказка PTT на пилюле — из настроек:** `keybind::shortcut_label(vk)` берёт
  реально назначенную кнопку (`Option<PttBind>` / `CURRENT_VK`), а не хардкод
  «ПРОБЕЛ»; меняется вместе с хоткеем в настройках.
- **Поле NODE ID и кнопка «Войти»** — 44 px высоты, радиус 10; поле — `Frame`
  с `fill(INSET)`, подпись 7.5 и `TextEdit` без рамки. Кнопки `primary_button`
  — 12 px, радиус 10.
- **Диск рации (`widgets/hero.rs`):** радиус `side * 0.40`; шкала — базовое
  кольцо + короткие насечки снаружи (крупные каждые 8-й), промежуточное кольцо
  0.84R, зелёная дуга уровня с точкой на конце, диск 0.68R со ступенчатым
  верхним светом (26 уменьшающихся кругов со сдвигом вверх), внутри — фиолетовое
  кольцо 0.30 диска, точка и скобка справа, «P2P» моноширинным ниже. Дымка в
  кольце рисуется `radial_ring` от 0 (уходит под диск) — иначе у края диска
  видна резкая кромка и диск читается «мишенью».
- **Правило брандмауэра ставится идемпотентно.** `netsh advfirewall firewall add
  rule name=DiscordRF` не считает повтором уже существующее правило с тем же
  именем: старый код добавлял новую запись при каждом запуске, и в брандмауэре
  копились десятки одинаковых. Теперь `setup_firewall_rules` сначала ищет в
  `show rule name=DiscordRF verbose` полный путь текущего exe и при совпадении
  ничего не делает; если правила для этого exe нет — удаляет все одноимённые
  (в том числе от старых копий с другим путём) и добавляет одно. Код возврата
  netsh как признак «правила нет» использовать нельзя: он равен 0 и когда
  правил не найдено, поэтому сравнение идёт по тексту вывода (путь ASCII
  переживает OEM-кодировку).
- **Git ownership (critical):** the checkout is owned by `vPC\mcdag`, but Claude runs
  over SSH as `vPC\Claude-Worker`, so plain `git` fails with "dubious ownership". Always
  prefix: `git -c safe.directory=C:/Claude-Exchange/DISCORDRF <cmd>`.
- **Identity:** the iroh `SecretKey` is pinned in `%APPDATA%\Albus\secret.key` (32 raw
  bytes) so the NodeID is stable across restarts. `presets::N0` does NOT persist the key.
  `my_id` is `FNV-1a(NodeID bytes)` — a routing alias, unrelated to encryption.
- **Anti-impersonation:** `iroh_service.rs` recomputes `hash(QUIC NodeID)` for every
  received packet and drops any message whose `sender_id` doesn't match. The QUIC NodeID
  is authenticated by iroh's TLS.
- **Эффекты — опцией, по умолчанию выключены.** У каждого входа свои тумблеры
  «Шумодав», «Затвор» и «AGC», плюс «Эхо» (AEC, отдельно от профилей) и общий
  «Лимитер передачи» в настройках звука (`input1_denoise`, `input1_gate`,
  `input1_agc`, `input1_aec`, `input2_denoise`, `input2_gate`, `input2_agc`,
  `input2_aec`, `tx_limiter` в `config.ini`). Профили «Голос»/«Инструмент» —
  пресеты, которые ставят все три тумблера разом; после ручной правки виден
  профиль «Свой». Причина, по которой
  всё по умолчанию выключено: у одного из собеседников тракт загонял тихую речь
  в гейт и AGC, и его почти не было слышно, хотя обратный канал звучал
  нормально. Сырой микрофон честнее; обработку включают по желанию.
- **DSP chain, передача:** linear resampler (`resample_linear`, preserves pitch
  across any card rate, работает всегда, у каждого входа свой `resample_pos`)
  → по тумблерам слота: AEC3 (эхо из колонок), затем шумодав RNNoise, затем
  noise gate (−44 dBFS) и/или `TransmitAgc` (цель −20 dBFS RMS, усиление
  −12…+24 дБ) → сумма входов,
  нормированная по числу звучащих → `Limiter` (−1 dBFS) при включённом
  «Лимитере передачи» → Opus 64/96/128 кбит/с. Выключенный эффект пропускается
  как есть; у инструмента обычно выключены все.
- **Шумодав — RNNoise на чистом Rust (`nnnoiseless`).** Затвор режет только
  тишину: клавиатура, кулер и улица выше −44 dBFS проходят вместе с речью.
  RNNoise различает речь и ровный шум по обученной модели и гасит шум под
  речью. Кадр модели 480 сэмплов (10 мс), поэтому наш кадр 960 режется на два;
  сэмплы подаются в масштабе i16 (×32768), модель обучена именно на нём.
  Состояние создаётся лениво при первом включённом кадре и сбрасывается при
  выключении тумблера — повторное включение начинает с чистой модели, а
  выключенный шумодав не держит в памяти ни модель, ни хвост. Первый кадр
  после включения содержит затухание модели (артефакт входа), это раз на
  включение.
- **Эхо-подавление — AEC3 на чистом Rust (`aec3_rs`).** Когда собеседники
  звучат в колонках, микрофон слышит их голоса и отправляет обратно — человек
  слышит сам себя с эхом. Затвор и шумодав тут не помогают: эхо идёт вместе с
  речью. AEC3 вычитает из микрофона то, что играют колонки: адаптивный фильтр
  подстраивается под акустику комнаты и задержку, а его подавитель гасит
  остаток. Опорный сигнал — моно-микс голосов собеседников из колбэка вывода
  (`out_scratch` после общей громкости, до монитора и гудка): монитор в опору
  не входит, иначе AEC вычитал бы из микрофона свой же голос. Карта не на
  48 кГц — опора ресэмплится в колбэке. Воркер забирает 960 сэмплов опоры
  каждый тик и режет их вместе с кадром на половинки AEC3 (480 сэмплов,
  10 мс); состояние создаётся лениво у каждого входа, при выключении тумблера
  выбрасывается. Слот с AEC прокачивается и молчащим входом — история
  фильтра не рвётся. Тумблер «Эхо» не входит в профили: он нужен только тем,
  кто слушает без наушников.
- **Битрейт Opus на передачу выбирается: 64 / 96 / 128 кбит/с.** Речь живёт на
  64, гитара — нет: на 64 кбит/с моно удары получают предэхо, а затухающие ноты
  «варблят». Поток один на оба входа, поэтому битрейт общий. Значение хранится
  в `config.ini` как `opus_bitrate`, неизвестное отбрасывается, а в микшере
  округляется до ближайшего из трёх (`OPUS_BITRATES`). `txbus` переписывает
  `encoder.bitrate_bps` только при смене — переключение действует без
  перезапуска потока и без изменения протокола (декодер каналы и битрейт
  узнаёт из самого Opus-пакета).
- **Один энкодер и один поток кадров на оба входа (`src/txbus.rs`).** Каждый
  аудиоколбэк только читает свой канал, применяет громкость слота, ресэмплит в
  48 кГц и кладёт сэмплы в кольцо на 4800 сэмплов (`try_send` в notify и
  `into_inner()`-локи — из колбэка ничего блокирующего). Поток `albus-tx` по
  абсолютному дедлайну раз в 20 мс забирает по 960 сэмплов на слот, недобор
  добивает последним сэмплом (нули посреди сигнала слышны щелчком), пустой слот
  — тишина. Почему не энкодер в колбэке: у двух карт свои часы, кадр запускался
  бы не по фазе, а AGC адаптировался бы с произвольной частотой.
- **Канал входа — «выключен», если вне диапазона.** `u32::MAX` = выключен, и
  индекс вне числа каналов тоже считается выключенным, без отката на канал 0:
  иначе моно-микрофон попал бы в обе дорожки. Эпоха (`InputBus::epoch`) на слот
  отсекает колбэк старого потока после горячей смены устройства; новый поток
  собирается с `next_epochs` и применяется только после успешной сборки
  (`commit_epochs`), чтобы неудачная попытка не оставляла без звука рабочий вход.
  Второй вход не откатывается на устройство по умолчанию: в инструмент молча
  пошёл бы микрофон. Слот без сэмплов дольше 250 мс гасит VU и молчит — выход
  карты из строя не делает первый вход вдвое тише.
- **DSP chain, приём:** миксовый `Limiter` тоже опционален (`rx_limiter`).
  `Limiter` на каждый голос **не ставится** — персональная
  громкость (слайдер в дБ, −30…+18) умножается на декодированный буфер, голоса
  складываются в общий микс, и уже микс проходит master gain + один `Limiter` (−1 dBFS).
  Порог пика у каждого голоса отдельно съедал персональный буст: при пиках 0.3 из
  +18 дБ оставалось ~+10, при 0.5 — ~+5.7, а если общая громкость уже стояла на 300%,
  персональная не добавляла почти ничего.
- **AGC на передаче (когда тумблер AGC включён) — это и есть «как в Discord»:** без него микрофон с речью на
  −20 dBFS уходил в эфир на −20 dBFS, и громкость добиралась уже у собеседника.
  Ниже −66 dBFS RMS усиление не трогается — тихая речь этого порога достигает,
  а шум в паузах до AGC не доходит, если включён затвор: он обнуляет его раньше.
  Вниз усиление идёт почти мгновенно (0.7 за колбэк), вверх — медленно (0.05):
  при медленном спаде первая доля громкой фразы проходила некомпрессированной,
  и в мониторинге это слышалось как «громко, затем резко тише».
  Огибающая RMS захватывается быстро (0.8), отпускает медленно (0.1).
- **Прослушивание своего тракта (self-monitor):** тумблер «Слышать свой тракт» в
  настройках звука. В `AudioMixer.monitor_buf` кладётся каждый собранный кадр
  (`txbus`) — то есть ровно то, что уйдёт в Opus: после включённых эффектов входа,
  суммы и лимитера, поэтому прослушивание не зависит от PTT. В колбэке воспроизведения он подмешивается к
  миксу **после** master gain (уровень монитора не зависит от громкости собеседников)
  и **до** миксового лимитера, с ресэмплингом 48 кГц → частота карты. Буфер ограничен
  400 мс, при включении чистится. Состояние сессионное, в `config.ini` не пишется.
  Задумано для баланса микрофона и гитары до созвона; через колонки микрофон
  поймает свой же голос — только наушники.
- **Микс-буфер у каждого собеседника свой:** `Peer.pcm` в `audio.rs`, кадры
  складываются поверх при выводе. Раньше все куски дописывались подряд в один
  `pcm_buffer` — когда говорили двое, речь шла друг за другом во времени, буфер рос,
  и его хвост выбрасывался: один голос глушил другого.
- **Default PTT key:** Mouse 4 (side button); global via `GetAsyncKeyState`, works in
  fullscreen games / minimized window.
- **Settings persistence:** `src/config.rs` reads/writes `%APPDATA%\Albus\config.ini`
  (plain `key=value`; a broken file is ignored). Devices are matched by **name** (cpal
  indices shift). `input2_device` missing, empty or equal to `input_device` means one
  stream reads both channels of one card; profiles are `input1_profile` /
  `input2_profile` (0 — голос, 1 — инструмент, прочее отбрасывается). `opus_bitrate`
  is 64000, 96000 or 128000 (anything else is dropped). Per-peer volume
  is stored as `peer_<id>_volume` keyed by the stable `my_id`. Stream rate is `video_fps` (30 or 60; missing or invalid means 60), viewer delay
is `video_buffer_ms` (0..5000, default 2000; larger values are clamped). `friends.rs` stores `%APPDATA%\Albus\friends.json` (serde_json). Files are
  written on change with a 400 ms throttle, never on exit.
- **`menu.rs` is dormant:** `handle_command` is defined and exported, but `main.rs`
  spawns no stdin-reader loop, so the `/mute`, `/vol`, `/mic`, `/devices` console
  commands are not currently reachable.
- **Звонки (`src/calls.rs`):** «Позвонить» только шлёт `CallInvite`; в комнату
  собеседник попадает после `CallAccept`. Миграции комнаты шлют `IrohSync` каждую
  секунду и вычищают молчащих >12 с, поэтому согласие держится не состоянием, а
  гейтом: `CallManager::is_blocked` отбрасывает `IrohSync`/`Heartbeat`/`AudioFrame`
  от собеседника в состоянии Outgoing/Incoming (35/45 с), а отказ/таймаут/отмена/
  завершение держат его вне комнаты `BLOCK_AFTER` = 1 час и дополнительно закрывают
  QUIC-соединение (`disconnect_peer`) — 45 с хватало ровно до следующего `IrohSync`,
  и завершённый звонок «возобновлялся» сам. Блок снимается явно: новым звонком
  (`start_outgoing`/`accept_incoming`/`on_accept`), кнопкой «Войти» или «Выйти из
  эфира». Свой отказ запоминается отдельно (`declined`, 45 с) и переживает блок:
  другу после отказа можно позвонить снова, а повторы Invite ещё 45 с получают
  `CallDecline`. Приглашение переотправляется раз в 1.5 с, `CallAccept` — трижды;
  одновременный вызов с двух сторон разрешается автоответом на встречное
  приглашение. Отклонение/«занято»/«не ответил» видны тостом; шапка звонка —
  оверлей `draw_call_overlay` с крупной красной кнопкой «Завершить звонок»
  («Отменить звонок» у исходящего), `Esc` делает то же самое. В активном
  разговоре карточки-оверлея нет — только компактная пилюля в правом верхнем
  углу (точка LIVE, ник, таймер, «Завершить»), она не закрывает трансляцию.
  Карточка «Принять/Отклонить» раньше не исчезала после ответа: методы звонков
  снимали фазу (`take_phase`) и клали её назад (`put_phase`), поэтому
  устаревшая `Incoming` затирала только что установленную `Active`. Теперь все
  переходы идут под одним замком, а `on_accept`/`on_decline`/`on_cancel`/`on_end`
  дополнительно сверяют `call_id` — иначе запоздавший `Accept` прошлого звонка
  поднимал новый. На входящем звонке
  окно один раз запрашивает внимание панели задач
  (`ViewportCommand::RequestUserAttention`), иначе свёрнутое приложение звонок
  «просто не слышно». Тон звонка синтезируется в колбэке воспроизведения
  (`mixer.ring_mode`: 1 — входящий, 2 — исходящий) после master gain и до
  миксового лимитера, поэтому не зависит от громкости собеседников. Отдельная
  причина «звонит у звонящего, у принимающего тишина» — `send_bytes`
  раньше молча возвращал Ok при отсутствующем или мёртвом соединении: теперь он
  выбрасывает закрытую запись (`Connection::close_reason`), переподключается и
  повторяет, а если датаграм не прошёл — шлёт сообщение надёжным uni-стримом
  (его читает `spawn_stream_receiver` рядом с приёмником датаграмм).
- **«Выйти из эфира» (`RoomManager::leave`):** красная кнопка в секции «Комната»
  (видна, когда в комнате есть собеседники) завершает/отменяет/отклоняет звонок,
  очищает участников, ставит флаг `left` и рвёт все QUIC-соединения
  (`disconnect_all`). Пока флаг стоит, `iroh_service` отбрасывает `IrohSync`,
  `Heartbeat` и `AudioFrame` — иначе автосинхронизация mesh возвращает всех в
  комнату за секунду. Снимается «Войти» или начатым/принятым звонком (`room.join`).
  До этого из активного звонка можно было выйти только закрытием приложения.
- **«Не запускается» = две разные причины.** exe несёт манифест
  `requireAdministrator`, поэтому из обычного терминала/без подтверждения UAC
  процесс получает «requires elevation» и молча не стартует. Если же UAC
  подтверждён, но окно не появилось — смотреть `%TEMP%\albus-panic.log`:
  в виртуалке/Hyper-V/RDP нет OpenGL 2.0, и glow падает с
  `egui_glow requires opengl 2.0+`. Теперь `run_native` не `unwrap`-ится:
  ошибка печатается и показывается `MessageBoxW` (то же окно упоминает
  `ALBUS_RENDERER=wgpu`). Feature `wgpu` включён в eframe, рендерер выбирается
  переменной окружения (по умолчанию glow). `pollster` подключён локальной
  заменой `vendor/pollster` через `[patch.crates-io]`: его `.crate` не скачивался,
  потому что реестр cargo на сборочной машине принадлежит другому пользователю.
  Если в такой VM и DX12-адаптера нет (`Failed to create wgpu adapter`), GUI там
  не поднять вовсе — приложение проверяется на обычной машине.
- **Онлайн-статус друзей — по пингу, а не по комнате.** `PresenceTracker`
  (`src/presence.rs`) держит `HashMap<NodeID, Instant>` с TTL 12 с: `spawn_ping_loop`
  раз в 4 с шлёт `PresencePing` друзьям без свежей отметки (отдельной задачей и с
  таймаутом 6 с — `send_bytes` сам поднимает соединение по NodeID, поэтому друг,
  только что запустивший приложение, загорается без ручного «Войти»). Любой
  входящий пакет отмечает узел живым по `pkt.sender_id` — это NodeID соединения
  из QUIC/TLS, а не поле пакета: релейный `IrohSync` несёт чужой `node_id`, и
  отмечать его нельзя. На пинг отвечаем `PresencePong`. `Heartbeat`/`IrohSync` для
  этого не годятся — они существуют только внутри комнаты. В панели друзей точка
  зелёная/серая; «Позвонить» офлайн-другу не гудит 35 с в пустоту, а сразу
  показывает тост «… не в сети».
- **Chat (DM):** session-only and in-memory (`src/chat.rs`, never persisted) — by
  design, so chat history cannot leak to disk. Friends-only: messages from non-friends
  are dropped. Transport is iroh **datagrams** with a reliable uni-stream fallback in
  `IrohP2pManager::send_bytes` (reconnect + retry when the map entry is dead or the
  datagram fails), so a lost datagram no longer loses the message. Text is
  capped to 1024 bytes on send (QUIC datagram MTU).
- **Unread chat:** `ChatLog` counts unread per NodeID. The badge lives on the
  «Чат» chip in the **«Эфир» header row**, not in the console footer — at the default
  window size the footer is clipped below the edge, so the badge was invisible.
  An open chat overlay calls `mark_read` every frame (before, the counter only grew,
  so the badge never cleared), and a rise in a friend's unread count while their
  chat is closed also shows a toast «Новое сообщение · <ник>» — the badge alone is
  easy to miss.
- **Video pipeline (Windows):** DXGI capture (BGRA, GPU) → `ID3D11VideoProcessor`
  converts/scales to NV12 on the GPU → hardware H.264 MFT → iroh → decoder MFT →
  NV12 → Video Processor → BGRA → egui texture. Bitrate is `pixels × fps ×
  bits_per_pixel` (Low 0.04 / Balanced 0.07 / Quality 0.10, capped 1.5–40 Mbps),
  so 1080p60 ≈ 8.7 Mbps and 1440p60 ≈ 18–22 Mbps. Capture runs at the monitor's
  native resolution — upscaling to a preset wastes bitrate.
- **AMF encoder quirk:** the AMD MFT ignores `MF_MT_FRAME_RATE` and computes rate
  control against 30 fps — at 60 fps the stream came out twice the requested
  bitrate. `win_mf::mean_bitrate_for()` compensates with `30/fps` for MFTs whose
  friendly name contains "amd"; re-check when the NVIDIA MFT is selected instead.
- **NVIDIA MFT outside a user session:** `ActivateObject` fails with
  `E_UNEXPECTED` (0x8000FFFF) in a non-interactive SSH session; AMF activates
  fine. Encoder checks must run interactively.
- **DXGI capture needs an interactive desktop.** Over SSH `ScreenCapture::new`
  returns "не найден экран"; use `tools/video_probe --capture` in the user's
  session (it reports the adapter and effective fps).
- **A fresh output sample per `ProcessOutput` call:** `win_decode.rs` and
  `win_mf.rs` allocate a new `IMFSample` for every call. Reusing one persistent
  sample makes the Microsoft H.264 decoder fail every frame with
  `ProcessOutput: CopyDecodedFrame failed (0x80004005)`.
- **The viewer shows frames on a delay (`video_buffer_ms`, default 2000).** Each
  `RemoteFrame` carries `arrived: Instant`; `RemoteQueue::pop_due(buffer, now)`
  releases the head only when `arrived + buffer <= now`. The queue is in display
  order, so a frame that arrives 1.5 s late holds its successors instead of being
  skipped — that is the whole point of the buffer: the network gets time to
  deliver. `0` means "show at once", as before. The cap is
  `buffer*fps + fps*2 + 30` frames (clamped to 96..600) and 96 MiB: it must cover
  the buffer plus the join burst of a whole GOP, or the queue would clear itself
  right after subscribing. On overflow the queue is dropped and the stream waits
  for a keyframe — P-frames without their references decode to garbage. The GUI
  decodes at most 4 frames per update so one slow frame cannot stall the UI.
- **Display is capped at the stream's own fps** (`show_period` in `video_session.rs`,
  taken from `StreamInfo.fps` — 30 or 60): preview and remote stream still push every
  frame through the decoder (a skipped P-frame breaks the chain) but only show that
  many per second, so a burst cannot flood the GPU readback and texture upload. The
  threshold is 75% of the period on purpose: at a 75 Hz repaint (13.3 ms per update) a
  hard 16.7 ms period skipped every second update and pinned the preview at ~32 fps
  even though each frame cost only ~4 ms. A frame that cannot go to the GPU right
  away (previous conversion still pending, or the rate check said "too early") is
  kept in `Renderer.queued` and retried on the next update instead of being dropped;
  a newer frame replaces it. Dropping it instead cost ~8 fps (52 instead of 60),
  because every readback that landed one update late threw away the frame after it.
- **Readback never blocks the GUI:** `Renderer` runs the GPU conversion
  (`converter.convert` → `copy_to_staging`, which also calls `Flush`) and collects the
  result on a later update with `read_back_nowait` (`D3D11_MAP_FLAG_DO_NOT_WAIT`;
  `DXGI_ERROR_WAS_STILL_DRAWING` yields `Ok(None)` and the frame is retried next
  update). A blocking `Map` waits for the video processor to finish — at 1080p that
  cost ~30 ms per frame and pinned the preview at ~30 fps even though the stream ran
  at 60. Do **not** call `CopyResource` inside the retry path: a fresh copy marks the
  staging texture busy again, `Map` answers "still drawing" forever and the preview
  never shows a frame. `pending_since` drops a readback after 500 ms as a safety valve.
- **Own preview runs at half resolution** (`Renderer::new(info, 2)`): the frame is
  still decoded at full size, but the GPU converter writes half-size BGRA, so the
  readback, `bgra_to_image` and texture upload cost 4× less — at 1080p those three
  stages alone held the preview at ~30 fps. Peer streams stay full-size
  (`Renderer::new(info, 1)`). `Renderer::timings()` feeds the "декод · чтение с GPU ·
  картинка" line under the broadcast button.
- **The audio reorder buffer must not wait for a lost datagram:** in `audio.rs`
  playback skips a gap (moves `entry.0` to the lowest buffered seq) as soon as 3 newer
  packets are queued. Waiting for the missing seq froze the playback until the buffer
  filled up and then dumped it in a burst — that is the "robot voice" one peer hears
  when its link drops datagrams.
- **Every peer needs its own Opus decoder, resampler position and reorder queue.**
  `create_playback_stream` used to feed all senders through one `OpusDecoder`;
  Opus is stateful (SILK/CELT), so interleaving two people's packets corrupted
  that state and the speech turned "robotic" as soon as a second voice joined.
  The receive state is now a per-sender `Peer` struct in `audio.rs`. Gaps are
  concealed by repeating the sender's last frame with a fade (up to 3 frames =
  60 ms), the prebuffer is 60 ms, and an overflowing jitter buffer is trimmed to
  80 ms out of 200 ms. A sender that restarts (seq back to 0) is re-armed on its
  own after ~25 consecutive "stale" packets, so it stays audible while others
  talk. Every 10 s the callback prints reception counters (buffer underruns,
  concealed frames, stream restarts, burst drops) — that log is what tells which
  side "robots": our playback or their link.
- **The resampler position must carry over as an offset from the next chunk's
  start** (`*position -= input.len()`, not `len - 1`). Subtracting one sample
  less shifted the phase by a sample on every audio callback, so a piece of the
  signal was dropped at each buffer boundary (an audible tick/buzz; worse with
  small buffers). This path runs constantly on 44.1 kHz cards — the QUAD-CAPTURE
  rig is 44100 in and out. The last sample is held past the end of a chunk
  instead of reading past the slice.
- **The playback stream's sample format must come from the card, never assumed.**
  `create_playback_stream` used to take the first config range that contains
  48 kHz and always build a stream from `f32`; on a device that only accepts
  `i16` (some USB/HDMI outputs, 8-channel virtual cards) cpal answers
  `BuildStreamError::StreamConfigNotSupported`, no stream opens, and the peer
  hears nothing at all. Now the candidates are f32@48k → i16@48k → the device's
  default config, and `build_playback<T>` is generic over the sample type (the
  jitter buffer and decoders stay in f32 and are converted at the end of the
  callback). The chosen format is printed with the device line.
- **The microphone stream must match the card's format too.** `create_input_stream`
  used to build every stream from `f32` no matter what the device accepted — on a
  mic that only takes `i16` the stream failed to open (or was built against a
  config it cannot use), and the microphone silently stayed dead while reception
  worked fine: exactly the "my friend can't hear me, but I hear him" case.
  Candidates are now `f32@48k → i16@48k → i32@48k →` the device's default config,
  and `build_input<T>` is generic over the sample type. Both the chosen format
  and the device are logged (`[АУДИО] Открыт микрофон: …`).
- **A saved device that has vanished falls back to the default one.** A USB mic
  unplugged between sessions left the app with no input at all: `stream_control.rs`
  now logs «Выбранное устройство не найдено … — берём … по умолчанию» and opens the
  system default instead of failing.
- **A multichannel endpoint gets the signal in two channels only.** On a
  virtual 7.1 output (`Динамики (7.1 Surround Sound)`, 8 channels) writing the
  same mono sample into all eight made the peer's speech a mush: the
  virtualizer mixes those eight identical copies with different delays, so
  comb filtering smears the voice. Candidates are now sorted by channel count
  (f32 before i16, then fewer channels), and the callback writes only the
  first two channels (`active_channels = channels.min(2)`), leaving the rest
  silent. Feeding LFE/center/surround with a copy of the front pair is never
  what a plain stereo signal should do.
- **Razer's virtual "7.1 Surround Sound" endpoint mangles `f32`.** This is a
  known cpal problem ([RustAudio/cpal#892](https://github.com/RustAudio/cpal/issues/892)):
  every cpal app (rodio, kira, Bevy) plays heavily distorted sound on that
  device while other outputs are clean, and Razer's own surround toggle makes no
  difference — the same stream in `i32` plays fine. So `i32` is a candidate
  format too, and on endpoints with more than two channels it is tried before
  `f32`; stereo endpoints keep `f32` first. The order is printed per attempt
  (`формат … не подошёл`), so the log shows which format the device did accept.
- **A silent "nothing plays" must be visible in the console.** `main.rs` used to
  swallow `create_playback_stream`/`create_input_stream` errors with `if let Ok`,
  so a failed stream looked like a working app that transmits but stays quiet.
  Both now print `[ОШИБКА] Динамики не открылись: …` / `Микрофон не открылся: …`.
- **Build stamp for stale copies:** `build.rs` exports `ALBUS_BUILD_STAMP` (the
  newest mtime under `src/` or `build.rs`), and startup prints
  `[СБОРКА] Albus <version> · сборка <stamp>`. Two peers must show the same
  stamp — an exe copied to another machine stays whatever it was, and that is
  the first thing to rule out when one side has "robot voice" or 30 fps video.
- **Stream rate is a setting:** `video_fps=30|60` in `config.ini`, picked in the
  broadcast panel before starting (default 60); it threads through
  `VideoSession::start(fps)` → `ScreenPublisher::start(fps)` → capture, encoder and
  pacing. The panel shows both numbers — "показ N · захват M FPS": a low *захват*
  means the capture/encode loop cannot keep up, a low *показ* with a healthy *захват*
  means the per-frame GPU readback + `bgra_to_image` + texture upload is the limit.
- **Viewer buffer is a display deadline, not a sender-side delay.** `video_buffer_ms`
  (slider in the «ЭКРАН» settings section, 0..5000 in 100 ms steps) is read every
  frame by the GUI and passed to `VideoLink::set_buffer_ms`; `try_recv_due()` then
  releases a frame only after it has sat in the queue that long. Changing it on the
  fly applies to frames already queued (the deadline is evaluated at release). The
  viewer toolbar shows it as «буфер N.N с». It is not a NACK/retransmission
  mechanism — a frame lost in the network is skipped, the stream waits for the
  next keyframe only when the queue overflows.
- **Decoder output is 1088 rows** (height aligned to 16) and its UV plane starts
  below; `repack_nv12()` strips the padding to exactly w×h before upload.
- **The Microsoft H.264 decoder MFT is synchronous** (no
  `IMFMediaEventGenerator`); `win_decode.rs` detects both sync and async MFTs and
  drives them accordingly.
- **Take `pSample` out of `MFT_OUTPUT_DATA_BUFFER` before matching the
  `ProcessOutput` result.** The struct is built with `ManuallyDrop`, so an early
  `return` on `MF_E_TRANSFORM_NEED_MORE_INPUT` — the normal end of every frame —
  leaked the output buffer that was allocated for that attempt. For the decoder
  that is w×h×3/2 (3 MB at 1080p) per frame: private bytes grew ~180 MB/s while
  streaming (sampled with `PrivateMemorySize64`, did not show in the working set
  until Windows made the pages resident). Both `win_decode.rs` and `win_mf.rs`
  now take the sample first and let it drop on every early return.
- **Take the input view back out of `D3D11_VIDEO_PROCESSOR_STREAM` after
  `VideoProcessorBlt`.** `pInputSurface` is a `ManuallyDrop` field and the struct
  has no `Drop`, so the input view created for every frame in
  `Converter::convert` was handed to the driver and never released — the driver
  kept a frame-sized allocation per call (~3 MB at 1080p), which showed up as
  growth both during a stream and across broadcast start/stop cycles (the
  driver frees it late). `win_d3d.rs` now `ManuallyDrop::take`s it on every
  `Blt` outcome. Headless check: `tools/video_probe/src/bin/cycles.rs`.
- **Probe crate:** `tools/video_probe` is a separate package because `build.rs`
  link args (the requireAdministrator manifest) leak into examples and such a
  process cannot start in an SSH session. Build with
  `cargo build --release --manifest-path tools/video_probe/Cargo.toml
  --target-dir target` and `ALBUS_NO_ADMIN_MANIFEST=1` (build.rs skips the
  manifest when that variable is set). Its `Cargo.lock` is a copy of the root
  one (the crates.io cache is read-only for `Claude-Worker`) and is gitignored.
  `src/bin/cycles.rs` is the start/stop leak finder: it creates and drops exactly
  what lives between «Начать трансляцию» and «Остановить» (encoder, decoder, D3D
  device, NV12 texture, converter) for N rounds and prints commit/working-set
  after each, with no GUI involved. `--enc`, `--dec`, `--gpu` keep one stage
  only; `--dev`, `--tex`, `--conv` cut inside the GPU path; `--upload`, `--blt`,
  `--copy` cut inside the per-frame work. Vary the frame count to tell a
  per-frame leak from a per-session one.
- **Self-preview feedback:** showing your own stream on the same monitor feeds
  the capture loop continuously — expected behaviour, not a decoder bug.
- **Video window:** own preview and remote streams are shown in a separate
  resizable egui viewport ("Albus — трансляция") with an in-window fullscreen
  toggle; remote streams open it automatically.
- **Assets are compiled in** via `include_bytes!` (icon/logo) — no runtime asset loading.
- Source comments and user-facing strings are **Russian (UTF-8)**. One known cosmetic
  bug: `build.rs` stores its error string as mojibake (double-encoded cp866).
- `Cargo.lock` is committed and must stay committed (binary app — do **not** gitignore it).
