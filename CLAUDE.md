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
  `iroh` 1.4 (QUIC), `tokio` (full), `serde` + `serde_json`, `bytes`.
  Windows-only: `windows` 0.62 (DXGI Desktop Duplication, D3D11, Media Foundation)
  for screen streaming; other platforms build without it (`src/video` is gated).

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
   Mixer levels/channels, PTT key and hands-free come from config; mic/playback streams
   open the saved devices by name (fallback: system default). Per-peer volumes restored.
6. Two mpsc channels:
   - `packet_tx/rx: Vec<u8>` — outbound encoded voice.
   - `playback_tx/rx: (u64, u32, Vec<u8>)` — inbound `(sender_id, seq, opus)`.
7. Spawn background tasks:
   - `iroh_service::spawn_iroh_receiver` — Iroh listener + full-mesh + relay + friends.
   - `broadcast::spawn_audio_broadcast` — fan-out voice over Iroh.
   - `MigrationManager::start_heartbeat_loop` — 1 s IrohSync / host failover.
8. On Windows: `video_link::VideoLink::spawn(p2p, rt.handle())` — the video
   transport (its own ALPN). It must be spawned with the runtime `Handle`, not
   bare `tokio::spawn` — at this point `main` has not entered the runtime yet.
   The GUI receives it via `RadioApp::attach_video(link, rt_handle)`.
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
| `src/chat.rs` | `ChatLog`: session-only DM history in memory (per-NodeID threads, unread counts) |
| `src/room.rs` | `RoomManager`: `Mutex<Vec<Peer>>`, peer upsert/remove, next-host election |
| `src/migration.rs` | `MigrationManager`: 1 s IrohSync broadcast, host failover |
| `src/audio.rs` | `UniversalAudioCapture`: CPAL capture/playback, linear resample, Opus, noise gate |
| `src/mixer.rs` | `AudioMixer`: dB boost (0..300%), soft-knee limiter, mute, volume, per-peer gain |
| `src/devices.rs` | `DeviceSelector`: enumerate input/output devices, find by name |
| `src/config.rs` | `AppConfig`: `%APPDATA%\Albus\config.ini` (devices, PTT, mixer, nick, per-peer volume, `video_fps`) |
| `src/keybind.rs` | `PttBind`: global Win32 `GetAsyncKeyState` hook thread |
| `src/firewall.rs` | `setup_firewall_rules()`: Windows firewall inbound UDP rule |
| `src/menu.rs` | `handle_command()`: console `/mute`, `/vol`, `/mic`, `/devices` (dormant — see below) |
| `src/network/mod.rs` | Re-exports `NetworkMessage`; declares `broadcast`, `iroh_service`, `message`, `p2p_iroh` |
| `src/network/p2p_iroh.rs` | `IrohP2pManager`: QUIC datagrams, connect by NodeID, `broadcast_bytes` |
| `src/network/iroh_service.rs` | `spawn_iroh_receiver`: Iroh listener, sender_id↔NodeID check, full-mesh, relay, friends |
| `src/network/broadcast.rs` | `spawn_audio_broadcast`: parallel voice fan-out over Iroh |
| `src/network/message.rs` | `NetworkMessage` enum + hand-rolled binary (de)serialization |
| `src/gui/mod.rs` | `RadioApp`: eframe window, "Cyber Slate" theme, 60 FPS, network-error banner |
| `src/gui/stream_control.rs` | Hot-swap audio devices at runtime |
| `src/widgets/audio_settings.rs` | Settings grid: VU meter, sliders, device lists |
| `src/widgets/info_panel.rs` | Header: logo, user ID, copyable NodeID, settings entry |
| `src/widgets/identity.rs` | "Мой NodeID" block: full ID + copy button/click-to-copy |
| `src/widgets/friends_panel.rs` | Friends list, incoming requests, add-friend, call/remove, chat button + unread |
| `src/widgets/chat_panel.rs` | DM chat: message bubbles, input, send over iroh datagrams |
| `src/widgets/ptt_button.rs` | Hands-free toggle, on-screen PTT, TX indicator |
| `src/video/mod.rs` | OS-independent: `VideoPreset`/`VideoTarget` (bitrate from pixel count), `StreamInfo`, `EncodedUnit` |
| `src/video/win_dxgi.rs` | DXGI Desktop Duplication capture; `next_nv12()` returns one frame as NV12 or `None` |
| `src/video/win_d3d.rs` | D3D11 device + `ID3D11VideoProcessor` BGRA↔NV12 conversion/scaling, staging readback |
| `src/video/win_mf.rs` | Async hardware H.264 MFT encoder (NVENC/AMF/QSV), CBR/GOP via `ICodecAPI` |
| `src/video/win_decode.rs` | H.264 decoder MFT — both async and sync paths; NV12 repack 1088→1080 |
| `src/video/publisher.rs` | `video-publisher` thread: capture at the chosen fps (30/60) → NV12 → encode → mpsc of `EncodedUnit`; `encode_fps` counter for diagnostics |
| `src/network/video_link.rs` | `discordrf-video/1`: incoming video connections, one uni-stream per frame, fan-out to viewers |
| `src/gui/video_session.rs` | `Renderer` (decode → BGRA → egui texture), `VideoSession` (own stream), `RemoteView` (peer stream) |
| `tools/video_probe/` | Separate probe crate: `--capture`, `--gpu`, `--loopback` headless checks |
| `src/widgets/room_panel.rs` | NodeID input (connect), participant cards, per-peer volume |
| `assets/icon.png`, `assets/logo.png` | Embedded via `include_bytes!` (monolithic binary) |
| `.github/workflows/macos.yml` | macOS Universal 2 build |

## Wire protocol (`src/network/message.rs`)

Hand-rolled binary framing — **not** serde/bincode on the wire. First byte is a tag.
Integers are **little-endian**; text fields are `len u16 LE` + UTF-8 bytes.

| Tag | Message | Layout (after tag) |
|-----|---------|--------------------|
| 0 | `Heartbeat` | `sender_id` u64 LE, `is_host` u8 |
| 3 | `AudioFrame` | `sender_id` u64 LE, `seq` u32 LE, `opus_data` (rest of payload) |
| 4 | `HostMigration` | `new_host_id` u64 LE |
| 5 | `IrohSync` | `sender_id` u64 LE, `is_host` u8, `len` u16 LE, `node_id` UTF-8 |
| 6 | `FriendRequest` | `sender_id` u64 LE, `node_id` str, `nick` str |
| 7 | `FriendAccept` | `sender_id` u64 LE, `node_id` str, `nick` str |
| 8 | `FriendRemove` | `sender_id` u64 LE, `node_id` str |
| 9 | `ChatMessage` | `sender_id` u64 LE, `text` str |

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

When a broadcast starts, `VideoLink::add_viewer` opens a connection to every
peer already in the room; viewers accept automatically and the GUI opens the
video window. Peers who join mid-stream are not subscribed yet.

Stopping a broadcast must call `VideoLink::stop_viewers()` (the GUI does it in
the same click that drops `self.video`). Each viewer connection lives inside a
send task that selects on a `watch` stop signal; when it fires, the task ends and
with it the QUIC connection. Without this every stop/start cycle left the old
connections alive and added a fresh one per peer — pinned memory that grew with
every restart, plus duplicated video traffic. `add_viewer` also de-duplicates by
NodeID. On the receiving side a finished stream drops that sender's queued frames.

## Key implementation details & gotchas

- **Git ownership (critical):** the checkout is owned by `vPC\mcdag`, but Claude runs
  over SSH as `vPC\Claude-Worker`, so plain `git` fails with "dubious ownership". Always
  prefix: `git -c safe.directory=C:/Claude-Exchange/DISCORDRF <cmd>`.
- **Identity:** the iroh `SecretKey` is pinned in `%APPDATA%\Albus\secret.key` (32 raw
  bytes) so the NodeID is stable across restarts. `presets::N0` does NOT persist the key.
  `my_id` is `FNV-1a(NodeID bytes)` — a routing alias, unrelated to encryption.
- **Anti-impersonation:** `iroh_service.rs` recomputes `hash(QUIC NodeID)` for every
  received packet and drops any message whose `sender_id` doesn't match. The QUIC NodeID
  is authenticated by iroh's TLS.
- **DSP chain:** adaptive noise gate (−44 dBFS) → linear resampler (`resample_linear`,
  preserves pitch across any card rate) → Opus 64 kbps → soft-knee limiter (−1.0 dBFS) →
  +18 dB logarithmic boost.
- **Default PTT key:** Mouse 4 (side button); global via `GetAsyncKeyState`, works in
  fullscreen games / minimized window.
- **Settings persistence:** `src/config.rs` reads/writes `%APPDATA%\Albus\config.ini`
  (plain `key=value`; a broken file is ignored). Devices are matched by **name** (cpal
  indices shift). Per-peer volume is stored as `peer_<id>_volume` keyed by the stable
  `my_id`. Stream rate is `video_fps` (30 or 60; missing or invalid means 60). `friends.rs` stores `%APPDATA%\Albus\friends.json` (serde_json). Files are
  written on change with a 400 ms throttle, never on exit.
- **`menu.rs` is dormant:** `handle_command` is defined and exported, but `main.rs`
  spawns no stdin-reader loop, so the `/mute`, `/vol`, `/mic`, `/devices` console
  commands are not currently reachable.
- **Chat (DM):** session-only and in-memory (`src/chat.rs`, never persisted) — by
  design, so chat history cannot leak to disk. Friends-only: messages from non-friends
  are dropped. Transport is iroh **datagrams** (same as friends/control), so delivery
  is best-effort with no ack/retry; a dropped datagram loses the message. Text is
  capped to 1024 bytes on send (QUIC datagram MTU).
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
- **Viewer queue is bounded:** `video_link.rs` holds at most 30 frames
  (`MAX_QUEUE`); on overflow it drops the queue and waits for the next keyframe —
  P-frames without their references decode to garbage. The GUI decodes at most
  4 frames per update so one slow frame cannot stall the UI.
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
- **A multichannel endpoint gets the signal in two channels only.** On a
  virtual 7.1 output (`Динамики (7.1 Surround Sound)`, 8 channels) writing the
  same mono sample into all eight made the peer's speech a mush: the
  virtualizer mixes those eight identical copies with different delays, so
  comb filtering smears the voice. Candidates are now sorted by channel count
  (f32 before i16, then fewer channels), and the callback writes only the
  first two channels (`active_channels = channels.min(2)`), leaving the rest
  silent. Feeding LFE/center/surround with a copy of the front pair is never
  what a plain stereo signal should do.
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
