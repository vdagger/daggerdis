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
  `iroh` v1 (QUIC), `tokio` (full), `rubato`, `igd`, `ureq`, `bincode`/`serde`.

## Build & run

### Windows

```powershell
cargo build --release
.\target\release\albus.exe            # optional arg: port; default binds 0.0.0.0:0 (ephemeral)
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
can register firewall rules, forward a UPnP port, and install a global PTT keyboard hook.
Launching from a normal terminal triggers a UAC prompt.

## Architecture (startup flow, `src/main.rs`)

1. `firewall::setup_firewall_rules()` — auto-add Windows inbound UDP rule.
2. Parse optional port arg; bind a std `UdpSocket` on `0.0.0.0:<port>` (0 = ephemeral).
3. `my_id = rand::random::<u64>()`.
4. Init Iroh P2P (`IrohP2pManager::new().await`) → publishes a `NodeID`.
5. `upnp::auto_forward_port()`, then `stun::get_external_addr()` for the public UDP addr
   (fallback to `ip_detector::get_external_ip()` over HTTP if STUN + Iroh both fail).
6. `NetworkManager::from_std(socket, iroh)` wraps the socket into Tokio.
7. Build `RoomManager`, `AudioMixer`, `UniversalAudioCapture`, `MigrationManager`.
   Mixer levels/channels, PTT key and hands-free come from `config::AppConfig`; the mic
   and playback streams open the saved devices directly (fallback: system default).
8. Two mpsc channels:
   - `packet_tx/rx: Vec<u8>` — outbound encoded voice.
   - `playback_tx/rx: (u64, Vec<u8>)` — inbound `(sender_id, opus)` for playback.
9. Spawn background tasks:
   - `receiver::spawn_network_receiver` — UDP recv loop.
   - `iroh_service::spawn_iroh_receiver` — Iroh listener + full-mesh.
   - `broadcast::spawn_audio_broadcast` — fan-out voice.
   - `MigrationManager::start_heartbeat_loop` — 1 s heartbeat / host failover.
10. `eframe::run_native("Albus Audio", ...)` → `gui::RadioApp`.

## File map

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | Dependencies (tokio, iroh, cpal, rusty-opus, egui/eframe, igd, winres) |
| `build.rs` | `winres` UAC manifest (`requireAdministrator`) on Windows |
| `src/lib.rs` | Module re-exports + `pub use types::Peer` |
| `src/main.rs` | Entry point: wire everything, spawn tasks, run eframe |
| `src/types.rs` | `Peer { id, address, is_host, last_heartbeat }` |
| `src/room.rs` | `RoomManager`: `Mutex<Vec<Peer>>`, peer upsert/remove, next-host election |
| `src/migration.rs` | `MigrationManager`: 1 s heartbeat loop, IrohSync broadcast, host failover |
| `src/audio.rs` | `UniversalAudioCapture`: CPAL capture/playback, linear resample, Opus, noise gate |
| `src/mixer.rs` | `AudioMixer`: dB boost (0..300%), soft-knee limiter, mute, volume |
| `src/devices.rs` | `DeviceSelector`: enumerate input/output devices, find by name |
| `src/config.rs` | `AppConfig`: `%APPDATA%\Albus\config.ini` persistence (devices, PTT, mixer) |
| `src/keybind.rs` | `PttBind`: global Win32 `GetAsyncKeyState` hook thread |
| `src/firewall.rs` | `setup_firewall_rules()`: Windows firewall inbound UDP rule |
| `src/upnp.rs` | `auto_forward_port()`: IGD port mapping on home routers |
| `src/stun.rs` | `get_external_addr()`: RFC 5389 client + NAT type classification |
| `src/ip_detector.rs` | `get_external_ip()`: HTTP fallback for public IP |
| `src/menu.rs` | `handle_command()`: console `/mute`, `/vol`, `/mic`, `/devices` (dormant — see below) |
| `src/network/mod.rs` | Re-exports `NetworkManager`, `NetworkMessage` |
| `src/network/manager.rs` | `NetworkManager`: Tokio UDP wrapper + iroh handle |
| `src/network/p2p_iroh.rs` | `IrohP2pManager`: QUIC datagrams, connect by NodeID, `broadcast_bytes` |
| `src/network/iroh_service.rs` | `spawn_iroh_receiver`: Iroh listener, auto full-mesh, relay |
| `src/network/broadcast.rs` | `spawn_audio_broadcast`: parallel voice fan-out (Iroh + UDP fallback) |
| `src/network/receiver.rs` | `spawn_network_receiver`: UDP recv loop (guards Windows error 10054) |
| `src/network/message.rs` | `NetworkMessage` enum + hand-rolled binary (de)serialization |
| `src/gui/mod.rs` | `RadioApp`: eframe window, "Cyber Slate" theme, 60 FPS |
| `src/gui/stream_control.rs` | Hot-swap audio devices at runtime |
| `src/widgets/audio_settings.rs` | Settings grid: VU meter, sliders, device lists |
| `src/widgets/info_panel.rs` | Header: logo, user ID, copyable NodeID |
| `src/widgets/ptt_button.rs` | Hands-free toggle, on-screen PTT, TX indicator |
| `src/widgets/room_panel.rs` | NodeID input, participant cards, per-peer volume |
| `assets/icon.png`, `assets/logo.png` | Embedded via `include_bytes!` (monolithic binary) |
| `.github/workflows/macos.yml` | macOS Universal 2 build |

## Wire protocol (`src/network/message.rs`)

Hand-rolled binary framing — **not** serde/bincode on the wire. First byte is a tag.
Integers are **little-endian except ports, which are big-endian** (`to_be_bytes`).

| Tag | Message | Layout (after tag) |
|-----|---------|--------------------|
| 0 | `Heartbeat` | `sender_id` u64 LE, `is_host` u8 |
| 1 | `JoinRequest` | `sender_id` u64 LE |
| 2 | `PeerListUpdate` | `count` u16 LE, then per peer: `id` u64 LE, `ip_type` u8 (0=v4, 1=v6), ip bytes, `port` u16 BE |
| 3 | `AudioFrame` | `sender_id` u64 LE, `opus_data` (rest of payload) |
| 4 | `HostMigration` | `new_host_id` u64 LE |
| 5 | `IrohSync` | `sender_id` u64 LE, `is_host` u8, `len` u16 LE, `node_id` UTF-8 |

## Key implementation details & gotchas

- **Git ownership (critical):** the checkout is owned by `vPC\mcdag`, but Claude runs
  over SSH as `vPC\Claude-Worker`, so plain `git` fails with "dubious ownership". Always
  prefix: `git -c safe.directory=C:/Claude-Exchange/DISCORDRF <cmd>`.
- **DSP chain:** adaptive noise gate (−44 dBFS) → linear resampler (`resample_linear`,
  preserves pitch across any card rate) → Opus 64 kbps → soft-knee limiter (−1.0 dBFS) →
  +18 dB logarithmic boost.
- **Default PTT key:** Mouse 4 (side button); global via `GetAsyncKeyState`, works in
  fullscreen games / minimized window.
- **Settings persistence:** `src/config.rs` reads/writes `%APPDATA%\Albus\config.ini`
  (plain `key=value`, no extra dependencies; a broken file is ignored). Devices are
  matched by **name** — cpal indices shift between runs. A saved device that is not
  present falls back to the system default and the saved name is kept. The file is
  written on change with a 400 ms throttle, never on exit.
- **`menu.rs` is dormant:** `handle_command` is defined and exported, but `main.rs`
  spawns no stdin-reader loop, so the `/mute`, `/vol`, `/mic`, `/devices` console
  commands are not currently reachable.
- **Assets are compiled in** via `include_bytes!` (icon/logo) — no runtime asset loading.
- Source comments and user-facing strings are **Russian (UTF-8)**. One known cosmetic
  bug: `build.rs` stores its error string as mojibake (double-encoded cp866).
- `Cargo.lock` is committed and must stay committed (binary app — do **not** gitignore it).
- `serialize_msg` in `message.rs` has a dead `_ => data.push(99)` arm (all variants are
  already matched).
