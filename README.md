# Albus — Serverless P2P Mesh Рация на Rust (2026)

Бессерверная голосовая рация на базе iroh, кодека Opus и отказоустойчивой миграции хоста.

---

## 🗺️ Реальная карта файлов проекта (Source Tree)

### Точка входа и ядро
* `src/main.rs` — инициализация: вызов STUN (`0.0.0.0:PORT`), старт `NetworkManager`, запуск потоков аудио, heartbeat-миграции и вызов GUI `eframe`.
* `src/lib.rs` — корневой файл библиотеки, объявляет и реэкспортирует публичные модули.
* `src/types.rs` — базовые типы данных (`Peer`: id, address, is_host, last_heartbeat).
* `src/room.rs` — `RoomManager`: потокобезопасный список участников комнаты, учет статуса хоста.

### Сеть и P2P Traversal (`src/network/` + `src/stun.rs`)
* `src/stun.rs` — чистая реализация STUN (RFC 5389). Пробивает исходящий NAT через Google/Cloudflare STUN и получает внешний `IP:Port`.
* `src/network/mod.rs` — реэкспорты сетевых компонентов.
* `src/network/manager.rs` — `NetworkManager`: управление асинхронным сокетом Tokio UDP (`send_audio_packet`, `send_packet`, `recv_audio_packet`).
* `src/network/message.rs` — **КАНОНИЧНЫЙ** бинарный протокол (`NetworkMessage`, `serialize_msg`, `deserialize_msg`):
  * `0: Heartbeat` — проверка пульса и роли хоста.
  * `1: JoinRequest` — запрос клиента на присоединение.
  * `2: PeerListUpdate` — рассылка актуального списка пиров сети (используется для встречного пробива NAT).
  * `3: AudioFrame` — фреймы Opus-аудио.
  * `4: HostMigration` — смена хоста при выходе текущего.
* `src/network/receiver.rs` — асинхронный цикл приёма UDP: читает сокет, парсит `deserialize_msg`, маршрутизирует команды в RoomManager и аудио в очередь воспроизведения.
* `src/network/broadcast.rs` — утилиты широковещательной рассылки по списку пиров.
* `src/ip_detector.rs` — резервное HTTP-определение внешнего IP.
* `src/protocol.rs` — *legacy* структура пакетов (будет удалена в ходе унификации протокола).

### Аудио-пайплайн
* `src/audio.rs` — `UniversalAudioCapture`: захват микрофона (`cpal`), ресемплинг в 48 кГц (`rubato`), кодирование/декодирование в Opus (`rusty-opus`), вывод в динамики.
* `src/devices.rs` — `DeviceSelector`: перечисление аудиоустройств ввода/вывода в ОС.
* `src/mixer.rs` — `AudioMixer`: программное микширование и регуляторы громкости.

### Графика и виджеты (`src/gui/`, `src/widgets/`)
* `src/gui/mod.rs` — `RadioApp`: главное окно egui.
* `src/gui/stream_control.rs` — горячий перезапуск потоков CPAL при смене устройств в GUI.
* `src/widgets/room_panel.rs` — ввод IP/порта, кнопка встречного пробива `🚀 Впустить участника (Пробить NAT)` для хоста, кнопка клиента, список участников со статусами.
* `src/widgets/audio_settings.rs` — выбор микрофона/динамиков, ползунки громкости.
* `src/widgets/info_panel.rs` — отображение ID пользователя и внешнего адреса `IP:Port`.
* `src/widgets/ptt_button.rs` — кнопка/индикатор PTT (Push-To-Talk).
* `src/keybind.rs` — глобальный перехват клавиш/мыши для рации.
* `src/menu.rs` — вспомогательное верхнее меню приложения.
* `src/migration.rs` — фоновый цикл Heartbeat и логика переизбрания хоста при аварийном отключении.
