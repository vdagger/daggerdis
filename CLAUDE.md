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

### Проверки и GUI-тесты из-под агента (`tools/agent/`)

`albus.exe` — консольная программа: обычный `Start-Process` без редиректа оставляет
унаследованный stdout у дочернего процесса, и вызывающий терминал ждёт EOF на пайпе
(выглядит как «запуск завис», хотя окно открыто). Поэтому для агентских запусков:

```powershell
pwsh -NoProfile -File tools\agent\build.ps1 -Mode check|test|clippy|release
pwsh -NoProfile -File tools\agent\albus.ps1 -Action start|stop|shot|click|keys|info
```

`albus.ps1` запускает exe с `-RedirectStandardOutput/-Error` (журналы и скриншоты в
`%TEMP%\albus-agent`), ставит окно на (40,40) 1120×700, снимает экран (`-Action shot`)
и шлёт клики/клавиши в абсолютных координатах экрана (`-Action click -X … -Y …`,
`-Action keys -Combo ctrl+m`). Скриншот смотреть через `read_image` — так проверяются
UI-правки без ручного запуска.

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
| `src/tray.rs` | Иконка в трее (свой поток, скрытое окно, меню «Открыть»/«Выйти») и всплывающие уведомления Windows; связи с GUI — атомики + HWND главного окна |
| `src/autostart.rs` | Автозапуск при входе в Windows: ключ `Run` в HKCU через `reg.exe`, сверка пути, чтобы старая запись не считалась своей |
| `src/chat.rs` | `ChatLog`: session-only DM history in memory (per-NodeID threads, unread counts) |
| `src/diagnostics.rs` | `RxStats` (счётчики приёма голоса в атомиках), `LinkLine`/`OwnVideo`/`RemoteVideo`, `ReportInput` + `render()` — текст отчёта для панели «Диагностика» и «Скопировать отчёт» |
| `src/calls.rs` | `CallManager`: голосовая комната на N (Idle/Active с `room_id`, Incoming). Приглашение зовёт в текущую комнату, повторное приглашение второго/третьего не блокируется; `pending` — кому отправлено и кто ещё не ответил, переотправка и таймауты; отказ блокирует адресно; `RoomMembers` отдаёт новому участнику список остальных; тон звонит только пока в комнате никого нет |
| `src/room.rs` | `RoomManager`: `Mutex<Vec<Peer>>`, peer upsert/remove, next-host election, флаг `left` («Выйти из эфира») |
| `src/migration.rs` | `MigrationManager`: 1 s IrohSync broadcast, host failover |
| `src/audio.rs` | `UniversalAudioCapture`: CPAL capture/playback, линейный ресэмплер, per-peer mix bus. Вход пишет сэмплы своего устройства в слоты `txbus`, вывод кладёт в `echo_ref` опору AEC; Opus-энкодера и DSP в колбэке больше нет |
| `src/txbus.rs` | `InputBus` (кольца на два входа, эпохи, watchdog) + поток `albus-tx`: сборка кадра 20 мс (960 сэмплов), эффекты входов по тумблерам (эхо-подавление AEC3, шумодав RNNoise, затвор и AGC независимо), лимитер передачи, монитор, Opus 64/96/128 кбит/с |
| `src/mixer.rs` | `AudioMixer` (per-peer dB gain, master gain, mute, VU, monitor своего тракта, кольцо опоры `echo_ref`, тумблеры `input1_denoise`/`input1_gate`/`input1_agc`/`input1_aec`/`input2_denoise`/`input2_gate`/`input2_agc`/`input2_aec`/`tx_limiter_enabled`/`rx_limiter_enabled` — все по умолчанию выкл., битрейт `opus_bitrate`), `SlotProfile` (пресет тумблеров), `TransmitAgc`, `Limiter` (−1 dBFS, на микс) |
| `src/devices.rs` | `DeviceSelector` (enumerate/find by name), `DeviceWatcher` (фоновый обход раз в 2 с), `DeviceSnapshot`/`DeviceChange`/`reselect`/`plan_device_update` — правила горячего подключения (выбор по имени, откат на умолчание) |
| `src/config.rs` | `AppConfig`: `%APPDATA%\Albus\config.ini` (devices включая `input2_device`, PTT, mixer, эффекты `input1_denoise`/`input1_gate`/`input1_agc`/`input1_aec`/`input2_denoise`/`input2_gate`/`input2_agc`/`input2_aec`/`tx_limiter`, nick, per-peer volume, `video_fps`, `video_buffer_ms`, `rx_limiter`; старые `mic_processing` и `input*_profile` читаются для совместимости) |
| `src/keybind.rs` | `PttBind`: global Win32 `GetAsyncKeyState` hook thread |
| `src/firewall.rs` | `setup_firewall_rules()`: Windows firewall inbound UDP rule for the current exe — идемпотентно (лишние одноимённые записи чистятся) |
| `src/menu.rs` | `handle_command()`: console `/mute`, `/vol`, `/mic`, `/devices` (dormant — see below) |
| `src/network/mod.rs` | Re-exports `NetworkMessage`; declares `broadcast`, `iroh_service`, `message`, `p2p_iroh`, `video_directory`, `video_link` |
| `src/network/p2p_iroh.rs` | `IrohP2pManager`: QUIC datagrams + uni-стримы (`send_bytes` с переподключением, `spawn_stream_receiver`), connect by NodeID, `broadcast_bytes`, `disconnect_peer`/`disconnect_all`, `link_stats()` — RTT/потери/окно/«напрямую или релей» по выбранному пути QUIC (для диагностики) |
| `src/network/iroh_service.rs` | `spawn_iroh_receiver`: Iroh listener, sender_id↔NodeID check, full-mesh, relay, friends, chat, звонки (гейт по `CallManager::is_blocked`) |
| `src/network/broadcast.rs` | `spawn_audio_broadcast`: parallel voice fan-out over Iroh |
| `src/network/message.rs` | `NetworkMessage` enum + hand-rolled binary (de)serialization |
| `src/gui/mod.rs` | `RadioApp`: состояние, `update`, тема и общие помощники; оверлеи и панели вынесены в `gui/` |
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
| `src/video/win_wgc.rs` | Захват окна через Windows Graphics Capture: свой `IGraphicsCaptureItemInterop`, `CreateFreeThreaded`-пул, кадр → `ID3D11Texture2D` → тот же конвертер |
| `src/video/win_windows.rs` | Список окон (`EnumWindows`, рамка без тени) + `clamp_rect`: пересечение с экраном и чётные размеры |
| `src/video/publisher.rs` | `video-publisher` thread: capture at the chosen fps (30/60) → NV12 → encode → mpsc of `EncodedUnit`; `encode_fps` counter for diagnostics |
| `src/network/video_directory.rs` | `VideoDirectory`: номер своей трансляции (`my_gen`) + анонсы соседей из IrohSync (`video_gen`), TTL 5 с; общая для миграции, сервиса приёма и видео |
| `src/network/video_link.rs` | `discordrf-video/1`: контроль `kind=2/3/4/5` (подписка, ответ, feed издателя, релей-подписка) в первом bi-стриме, окно недавних кадров и relay под одним замком (180 / 64 МиБ, отдача с последнего ключевого не старше 500 мс, `force_keyframe` по подписке), релей хоста без ре-энкода, маршрутизация зрителя, очередь зрителя со сдвигом показа на буфер (не глубже `буфер + 24` кадра) |
| `src/gui/video_session.rs` | `Renderer` (decode → BGRA → egui texture), `VideoSession` (own stream), `RemoteView` (peer stream) |
| `tools/video_probe/` | Separate probe crate: `--capture`, `--gpu`, `--loopback` headless checks |
| `tools/agent/` | Скрипты для агента: `albus.ps1` (запуск GUI с редиректом stdio, скриншоты, клики, клавиши, позиционирование окна), `build.ps1` (check/test/clippy/release с журналом), `group.ps1` (N экземпляров с взаимной дружбой: свой `%APPDATA%`, NodeID из журнала, friends.json до запуска — для проверки группы) |
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
| 10 | `CallInvite` | `sender_id` u64 LE, `room_id` u32 LE, `nick` str — приглашение в комнату |
| 11 | `CallAccept` | `sender_id` u64 LE, `room_id` u32 LE — вошёл в комнату |
| 12 | `CallDecline` | `sender_id` u64 LE, `room_id` u32 LE, `reason` u8 (0 отклонён / 1 занято / 2 таймаут) |
| 13 | `CallCancel` | `sender_id` u64 LE, `room_id` u32 LE — приглашение отменено |
| 14 | `CallEnd` | `sender_id` u64 LE, `room_id` u32 LE — участник вышел из комнаты |
| 15 | `PresencePing` | `sender_id` u64 LE — «ты в сети?»; пинг живёт вне комнаты |
| 16 | `PresencePong` | `sender_id` u64 LE — ответ на пинг |
| 17 | `RoomMembers` | `sender_id` u64 LE, `room_id` u32 LE, `count` u16 LE, затем `count` × (`len` u16 LE + NodeID) — список комнаты новому участнику, чтобы mesh собрался целиком |
| 18 | `RoomChat` | `sender_id` u64 LE, `room_id` u32 LE, `nick` str, `text` str — общее сообщение комнаты (0 — эфир без созвона) |

### Video wire format (`src/network/video_link.rs`)

Video uses a **separate iroh connection** with ALPN `discordrf-video/1`: QUIC
congestion control is per-connection, so video cannot starve voice datagrams.
Each frame is its own **unidirectional stream** — a lost packet delays one frame,
not the whole GOP. Little-endian 32-byte header, then optional SPS/PPS and the
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
| 28 | u32 | `frame_seq` — номер кадра у отправителя: по нему виден разрыв и находится досыл |
| 32 | bytes | SPS/PPS (if flag 2), then H.264 |

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

**Late joiners start from a keyframe.** `publish` keeps a ring of the last 180
frames / 64 MiB (`Recent`) keyed by an in-memory `seq`; `handoff(max_age)`
snapshots it from the last keyframe **under the mutex `publish` uses**, and drops
the tail when that keyframe is older than 500 мс — иначе зритель навсегда отставал
бы от живого потока. По подписке издатель просит у энкодера свежий IDR
(`CODECAPI_AVEncVideoForceKeyFrame`), поэтому показ начинается сразу с живого
кадра; если ключевого в окне нет, задача ждёт следующего. Broadcast — 240
кадров, после `Lagged` — новый handoff вместо продолжения с P-кадра.

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
  rule name=DiscordRF` не считает дублем правило с тем же именем — старый код
  добавлял новую запись при каждом запуске. Теперь `setup_firewall_rules` ищет в
  `show rule name=DiscordRF verbose` полный путь текущего exe и при совпадении
  ничего не делает; если правила для этого exe нет — удаляет все одноимённые и
  добавляет одно. Код возврата netsh не годится как признак «правила нет»: он 0 и
  когда правил не найдено, поэтому сравнение идёт по тексту (путь ASCII переживает
  OEM-кодировку).
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
is `video_buffer_ms` (0..5000, default 400; larger values are clamped), preset is
`video_preset` (`low`/`balanced`/`quality`; неизвестное отбрасывается). `friends.rs` stores `%APPDATA%\Albus\friends.json` (serde_json). Files are
  written on change with a 400 ms throttle, never on exit.
- **Голосовая комната (`src/calls.rs`) — это и есть «звонок», на N человек.**
  `Active { room_id, pending }` вместо пары «звонящий/принявший»: приглашение
  зовёт в текущую комнату, поэтому второй и третий друг не получают «вы уже в
  звонке», а приглашаются в ту же комнату; `pending` хранит, кому отправлено и
  кто ещё не ответил (переотправка раз в 1.5 с, таймаут 35 с). `Incoming` —
  мы приглашены и ещё не ответили; Enter входит, Esc отклоняет. Отказ блокирует
  только этого узла (`blocked`/`declined`, адресно), а не комнату: один отказался
  — остальные продолжают разговор, и комната закрывается только когда в ней
  никого не осталось. «Завершить» = выйти из комнаты: остальные остаются друг у
  друга, `room.leave()` не даёт `IrohSync` вернуть нас, соединения рвутся.
  Два одновременных приглашения разрешаются переходом в комнату пригласившего
  (свои приглашения отменяются — у них другой `room_id`).
- **Хост — наименьший id среди соседей и себя** (`room.host_id_with_me`, раз в
  секунду). Флаг `is_host` в пакетах не используется: он выставлен у всех, и
  раньше каждый считал хостом себя — для видео это значило, что трансляцию не
  смотрел никто (хост не подписывается на чужой поток).
- **Уход и mesh.** `leave_voice` зовёт `announce_leave` (`CallEnd` всем в
  комнате) до разбора фазы: в ветке `Idle` (комната собрана одним mesh) уход
  иначе не объявлялся, и человека держали «молчит» до таймаута. Получатель по
  `CallEnd` сразу убирает его из `RoomManager`; `start_outgoing` не блокируется
  записью без heartbeat ≥ 6 с. Пока мы в эфире, `migration.rs` раз в секунду
  дотягивается до участников (`p2p.ensure_connected`) и только потом рассылает
  `IrohSync`; `on_accept`/`accept_incoming` регистрируют собеседника
  (`register_peer`) — иначе после обрыва узел «в комнате» без людей.
- **Новый участник узнаёт остальных из `RoomMembers` (тег 17).** Он соединён
  только с пригласившим, а остальные о нём не знают — без списка комнаты mesh не
  собирается, и человек слышит одного из трёх. Пригласивший в ответ на
  `CallAccept` отправляет список NodeID, получатель дотягивается до каждого;
  список принимается только если `room_id` совпадает с нашей комнатой (его знают
  лишь приглашённые). Дальше mesh достраивается обычным `IrohSync`.
- **Потерянный кадр голоса восстанавливается FEC/PLC, а не повтором.** Повтор
  последнего кадра — это и есть «робот»: одна и та же волна дважды. `txbus`
  включает `use_inband_fec` и `packet_loss_perc = 8` (Opus кладёт копию
  предыдущего кадра в следующий пакет, SILK/hybrid). На приёме дырка в `seq`
  закрывается `decode_fec(следующий_пакет)`, затем PLC (`decode(&[])` —
  продолжение тона и спектра), и только если декодер ещё ничего не слышал —
  затухающий повтор. Восстановленные FEC кадры видны в диагностике отдельным
  счётчиком (`RxStats::fec`).
- **«Выйти из эфира» (`RoomManager::leave`):** кнопка «Завершить/Выйти» завершает
  звонок, очищает участников, ставит флаг `left` и рвёт соединения; пока флаг
  стоит, `iroh_service` отбрасывает `IrohSync`/`Heartbeat`/`AudioFrame`. Снимается
  «Войти» или принятым звонком (`room.join`).
- **«Не запускается» = две разные причины.** exe несёт манифест
  `requireAdministrator`, поэтому из обычного терминала/без подтверждения UAC
  процесс получает «requires elevation» и молча не стартует. Если же UAC
  подтверждён, но окно не появилось — смотреть `%TEMP%\albus-panic.log`:
  в виртуалке/Hyper-V/RDP нет OpenGL 2.0, и glow падает с
  `egui_glow requires opengl 2.0+`. Теперь `run_native` не `unwrap`-ится:
  ошибка печатается и показывается `MessageBoxW` (то же окно упоминает
  `ALBUS_RENDERER=wgpu`). Feature `wgpu` включён в eframe, рендерер выбирается
  переменной окружения (по умолчанию glow). `pollster` подключён локальной
  заменой `vendor/pollster` через `[patch.crates-io]` (его `.crate` не скачивался:
  реестр cargo принадлежит другому пользователю).
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
- **Chat (DM и комната):** session-only, in memory (`src/chat.rs`), never
  persisted — history cannot leak to disk. DM is friends-only (strangers dropped);
  room chat (`RoomChat`, tag 18) goes to every room address and is accepted only
  from a room member while we are in the air, `room_id` 0 = общий эфир; history is
  per-room and cleared when the room changes. Transport is iroh **datagrams** with
  a reliable uni-stream fallback in `send_bytes` (reconnect + retry), text is
  capped to 1024 bytes on send (QUIC datagram MTU).
- **Unread chat:** `ChatLog` counts unread per NodeID, `RoomChat` — одно число на
  комнату; бейдж на чипе «Чат» в шапке «Эфира» (в подвале он обрезался за краем).
  Открытый оверлей зовёт `mark_read` каждый кадр, а рост непрочитанного в
  закрытой переписке даёт тост «Новое сообщение · <ник>» — бейдж легко пропустить.
- **Video pipeline (Windows):** DXGI capture (BGRA, GPU) → `ID3D11VideoProcessor`
  converts/scales to NV12 on the GPU → hardware H.264 MFT → iroh → decoder MFT →
  NV12 → Video Processor → BGRA → egui texture. Bitrate is `pixels × fps ×
  bits_per_pixel` (Low 0.05 / Balanced 0.10 / Quality 0.16, пол 2.5 и потолок
  40 Мбит/с), so 1080p60 ≈ 6 / 12 / 20 Mbps, а окно 1108x610x30 — 2.5 / 2.5 /
  3.2 Мбит/с; пресет выбирается в карточке трансляции
  (`video_preset`, «Авто» — по разрешению источника). Энкодер просит CABAC,
  0 B-кадров, «качество/скорость» 50 и VBV (пик 1.25 среднего, буфер 0.5 с);
  NVIDIA MFT не принимает `AVEncCommonRealTime` и `DefaultBPictureCount` — это
  видно строкой `ICodecAPI: [...]`, поток не рвётся. GOP 1 с (2 с ниже 4 Мбит/с):
  зритель получает свежий IDR по подписке, а не ждёт планового. Битрейт
  **адаптивный**: потери feed-канала опускают его
  (`watch_feed_bitrate`, вниз 15 %/с, вверх после 5 чистых секунд). Источник —
  весь экран или одно окно: прямоугольник окна вырезается из кадра стола через
  `VideoProcessorSetStreamSourceRect`; через WGC (`win_wgc.rs`) окно снимается
  независимо от перекрытия, иначе работает вырезка из экрана. Измерено на
  1080p60: захват 60 FPS, зритель 58-61 FPS.
- **AMF encoder quirk:** the AMD MFT ignores `MF_MT_FRAME_RATE` and computes rate
  control against 30 fps — at 60 fps the stream came out twice the requested
  bitrate; `win_mf::mean_bitrate_for()` compensates with `30/fps` for "amd" MFTs.
- **DXGI capture needs an interactive desktop.** По SSH `ScreenCapture::new`
  отвечает «не найден экран»; проверять `tools/video_probe --capture` в сессии.
- **A fresh output sample per `ProcessOutput` call:** `win_decode.rs` and
  `win_mf.rs` allocate a new `IMFSample` for every call. Reusing one persistent
  sample makes the Microsoft H.264 decoder fail every frame with
  `ProcessOutput: CopyDecodedFrame failed (0x80004005)`.
- **The viewer shows frames on a delay (`video_buffer_ms`, default 400).** Each
  `RemoteFrame` carries `arrived: Instant`; `RemoteQueue::pop_due(buffer, now)`
  releases the head only when `arrived + buffer <= now`. 2 с по умолчанию были
  задержкой поверх качества: зритель игры видел происходящее на два такта позже
  голоса. The cap is
  `buffer*fps + 24` frames (clamped to 40..600) and 96 MiB — глубже очередь не
  растёт, потому что приход и показ идут с одной частотой, и лишние кадры стали
  бы постоянным отставанием. On overflow the queue is dropped and the stream
  waits for a keyframe — P-frames without their references decode to garbage.
  The GUI decodes at most 4 frames per update so one slow frame cannot stall the UI.
- **Показ не ограничивается искусственным периодом.** Период `0.75·(1/fps)`
  выбрасывал кадр, если апдейт приходил на 12.5 мс при 74 Гц (13.5 мс), — зритель
  видел 44-46 из 60. Потолок задаёт сам поток, а декодированные кадры ждут в
  очереди `Renderer.queued` (3 кадра), поэтому два кадра в одном апдейте не
  теряются. `take_diag()` отдаёт «ждали GPU / выброшено» для журнала раз в 5 с.
- **Readback never blocks the GUI:** `Renderer` runs the GPU conversion
  (`converter.convert` → `copy_to_staging`, which also calls `Flush`) and collects the
  result on a later update with `read_back_nowait` (`D3D11_MAP_FLAG_DO_NOT_WAIT`;
  `DXGI_ERROR_WAS_STILL_DRAWING` yields `Ok(None)` and the frame is retried next
  update). A blocking `Map` waits for the video processor to finish — at 1080p that
  cost ~30 ms per frame. Do **not** call `CopyResource` inside the retry path: a fresh copy marks the
  staging texture busy again, `Map` answers "still drawing" forever and the preview
  never shows a frame. `pending_since` drops a readback after 500 ms as a safety valve.
- **Own preview runs at half resolution** (`Renderer::new(info, 2)`): the frame is
  still decoded at full size, but the GPU converter writes half-size BGRA, so the
  readback, `bgra_to_image` and texture upload cost 4× less. Peer streams stay full-size
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
  The receive state is now a per-sender `Peer` struct in `audio.rs`. Gaps up to
  3 frames (60 ms) are restored by FEC/PLC (see below), the prebuffer is 60 ms, and
  an overflowing jitter buffer is trimmed to 80 ms out of 200 ms. A sender that restarts (seq back to 0) is re-armed on its
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
- **Hot-plug: device lists are refreshed in the background and the selection lives
  by name.** `devices::DeviceWatcher` re-enumerates every 2 s on its own thread
  (`albus-devices`) and hands a `DeviceSnapshot` to the GUI; `refresh_devices()`
  feeds it through the pure `devices::plan_device_update`, which remaps each
  selection **by name**, falls back to the *current* default device when the
  chosen one is gone, and returns the added/removed lists for the log and the
  toast. Because indices shift whenever a card is plugged or unplugged,
  `reconcile_inputs` and the output switch compare `prev_*_name`, not indices —
  otherwise a reordered list restarted the stream on the very same device. An
  empty snapshot (cpal sees nothing while the driver reloads) is ignored, and the
  second input drops to «То же устройство» instead of falling back to the default
  mic (that would silently feed a microphone into the instrument slot).
  Windows `pnputil /disable-device <instance>` is a usable stand-in for unplugging
  a *driver* device; disabling an already-active endpoint with
  `Disable-PnpDevice` does **not** change its WASAPI state (it stays `ACTIVE` in
  `MMDevices`), so it cannot be used to fake a removal.
- **Diagnostics panel: the numbers come from QUIC and from the audio callback, not
  from pings.** `p2p_iroh::link_stats()` reads the *selected* QUIC path
  (`Connection::paths()` → `is_selected()`, `rtt()`, `lost_packets`, `cwnd`,
  `bandwidth_estimate`, `is_relay()`), so RTT is the same value the retransmission
  timers use and it exists for every connection (the presence ping only reaches
  friends that have been silent). It takes `try_lock`: the connection map lives
  under `tokio::sync::Mutex` while the GUI is synchronous, and skipping one frame
  is cheaper than blocking. Voice-reception counters are atomics in
  `diagnostics::RxStats`, written from the playback callback — they replaced the
  old per-10-s local variables, and the console log now prints the delta between
  snapshots. `ReportInput` + `diagnostics::render()` produce the text for both the
  panel and «Скопировать отчёт» (clipboard + console), so the screen and the
  copied report cannot diverge. The panel is read-only on purpose: a second path
  that changes audio/video next to the settings overlay would drift from it.
- **A multichannel endpoint gets the signal in two channels only.** On a
  virtual 7.1 output (`Динамики (7.1 Surround Sound)`, 8 channels) writing the
  same mono sample into all eight made the peer's speech a mush: the
  virtualizer mixes those eight identical copies with different delays, so
  comb filtering smears the voice. Candidates are now sorted by channel count
  (f32 before i16, then fewer channels), and the callback writes only the
  first two channels (`active_channels = channels.min(2)`), leaving the rest
  silent: LFE/центр/тыл копией фронта не кормим. У виртуального «7.1» Razer
  ломается `f32` (cpal#892): правило выбора — `audio::plan_output_candidates`
  (для таких имён i32 → i16 → f32, формат важнее каналов), ручной
  `output_format` в настройках звука, `u8` тоже поддержан; частота — 48 кГц
  или своя.
- **Build stamp:** `build.rs` отдаёт `ALBUS_BUILD_STAMP` (самый свежий mtime в
  `src/`), при старте печатается `[СБОРКА] Albus <version> · сборка <stamp>`.
  У двух узлов штамп обязан совпадать — экземпляр, скопированный на другую
  машину, остаётся старым, и это первое, что проверяют при «роботе» или 30 fps.
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
  viewer toolbar shows it as «буфер N.N с». Потерянный кадр зритель просит
  досылом (kind 6, из кольца того, кто отдаёт), очередь вставляет его по номеру;
  если кадра нет совсем — пропуск, ключевой ждём при переполнении.
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
  `Blt` outcome. Проверка утечек: `tools/video_probe`.
- Source comments and user-facing strings are **Russian (UTF-8)**. One known cosmetic
  bug: `build.rs` stores its error string as mojibake (double-encoded cp866).
- `Cargo.lock` is committed and must stay committed (binary app — do **not** gitignore it).










