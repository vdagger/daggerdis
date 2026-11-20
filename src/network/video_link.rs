//! Видеотранспорт.
//!
//! Отдельное iroh-соединение с ALPN `discordrf-video/1`: у каждого QUIC-соединения
//! свой congestion control, поэтому видео не душит голос. Один uni-стрим на кадр —
//! потеря пакета задерживает только один кадр, без head-of-line блокировки
//! между кадрами.
//!
//! Соединение открывает зритель, а к хосту — издатель со своим потоком. Первый
//! bi-стрим — управление; первый байт говорит, что это:
//! ```text
//! kind=2  зритель → издатель  u32 gen
//! kind=3  ответ               u8 status (0 — принято, 1 — отказ), u32 gen
//! kind=4  издатель → хост     u32 gen
//! kind=5  зритель → хост      u32 gen, u16 len, NodeID издателя
//! ```
//! Дальше по этому соединению идут кадры uni-стримами. Выход зрителя виден по
//! `Connection::closed()`: свой control-стрим клиент закрывает сразу после запроса.
//!
//! Топология звездой: издатель отдаёт ОДИН поток хосту комнаты (kind=4), хост
//! раскидывает те же сжатые байты всем зрителям (kind=5) и в своё окно — без
//! ре-энкода. Отдача издателя = 1 × битрейт, хоста = N × битрейт; лишний хоп
//! съедает буфер показа зрителя. Если хост сам вещает, зрители идут к нему
//! напрямую (kind=2): два потока в одном окне не уживаются. Фолбэка на прямую
//! передачу нет — сборки обязаны совпадать по штампу.
//!
//! Формат кадра (little-endian):
//! ```text
//! u8  kind (1)
//! u8  flags (1 = keyframe, 2 = есть SPS/PPS)
//! u16 seq_len
//! u32 width
//! u32 height
//! u32 fps
//! i64 pts_100ns
//! u32 len
//! [SPS/PPS][H.264 Annex-B]
//! ```

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::Connection;
use tokio::sync::{broadcast, watch};
use tokio::time::timeout;

use super::p2p_iroh::IrohP2pManager;
use super::video_directory::VideoDirectory;
use crate::room::RoomManager;
use crate::video::{EncodedUnit, StreamInfo};

pub const VIDEO_ALPN: &[u8] = b"discordrf-video/1";

/// Верхняя граница одного кадра: 1440p-кейфрейм с запасом.
const MAX_FRAME: usize = 8 * 1024 * 1024;

const KIND_FRAME: u8 = 1;
const KIND_SUBSCRIBE: u8 = 2;
const KIND_ACK: u8 = 3;
/// Издатель → хост: «отдаю тебе свой поток».
const KIND_FEED: u8 = 4;
/// Зритель → хост: «отдай мне поток этого издателя».
const KIND_RELAY: u8 = 5;

/// Управляющий пакет — вид, поколение и 64-символьный NodeID.
const CONTROL_LIMIT: usize = 256;
/// NodeID iroh в текстовом виде: 32 байта hex.
const NODE_ID_LEN: usize = 64;

const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Не долбим издателя или хост, который только что отказал или не ответил.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// Сколько кадров помнит окно издателя для «хвоста с ключевого». Broadcast
/// вмещает столько же: подписчик, не успевающий писать в сеть, при `Lagged`
/// просто заново берёт хвост, а не продолжает с P-кадра без опоры.
const BROADCAST_FRAMES: usize = 240;
const RECENT_FRAMES: usize = 180;
const RECENT_BYTES: usize = 64 * 1024 * 1024;

/// Буфер показа по умолчанию, мс (настройка `video_buffer_ms`).
pub const DEFAULT_BUFFER_MS: u32 = 2000;
/// Очередь: `buffer*fps + GOP на догон + запас`, не меньше 96 кадров и
/// не больше 600; отдельный байтовый предел на 4K-потоки.
const MIN_QUEUE: usize = 96;
const MAX_QUEUE: usize = 600;
const QUEUE_BYTES: usize = 96 * 1024 * 1024;

/// Пришедший по сети кадр.
pub struct RemoteFrame {
    pub from: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sequence_header: Vec<u8>,
    pub unit: EncodedUnit,
    /// Когда кадр пришёл: от неё считается дедлайн показа.
    pub arrived: Instant,
}

/// Очередь пришедших кадров для GUI зрителя.
struct RemoteQueue {
    frames: VecDeque<RemoteFrame>,
    bytes: usize,
    /// Часть P-кадров потеряна: декодировать нечего, ждём ключевой кадр.
    resync: bool,
}

impl RemoteQueue {
    fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            resync: false,
        }
    }

    fn frame_cost(frame: &RemoteFrame) -> usize {
        frame.unit.data.len() + frame.sequence_header.len()
    }

    /// Кладёт кадр в очередь, не давая ей расти без предела. `false` —
    /// кадр выброшен (ждём ключевой), очередь очищена.
    fn push(&mut self, frame: RemoteFrame, buffer: Duration) -> bool {
        if frame.unit.keyframe {
            self.resync = false;
        } else if self.resync {
            return false;
        }
        let fps = frame.fps.max(1) as usize;
        let buffer_frames =
            (fps as u64 * buffer.as_millis() as u64 / 1000) as usize;
        // Полный GOP на догон (fps*2 — интервал ключевых кадров 2 с), сам
        // буфер и запас. Старый предел в 30 кадров при буфере 2 с чистил
        // очередь сам, ещё до показа.
        let frames_cap = (buffer_frames + fps * 2 + 30).clamp(MIN_QUEUE, MAX_QUEUE);
        let cost = Self::frame_cost(&frame);
        if self.frames.len() >= frames_cap || self.bytes + cost > QUEUE_BYTES {
            self.frames.clear();
            self.bytes = 0;
            self.resync = true;
            if !frame.unit.keyframe {
                return false;
            }
            self.resync = false;
        }
        self.bytes += cost;
        self.frames.push_back(frame);
        true
    }

    /// Кадр из головы, если его буфер уже истёк. Порядок очереди = порядок
    /// показа, поэтому опоздавший кадр придерживает следующие и сеть успевает
    /// отдать остальные. Дедлайн считается в момент выдачи — смена настройки
    /// действует и на уже пришедшие кадры.
    fn pop_due(&mut self, buffer: Duration, now: Instant) -> Option<RemoteFrame> {
        let head = self.frames.front()?;
        if head.arrived + buffer > now {
            return None;
        }
        let frame = self.frames.pop_front()?;
        self.bytes = self.bytes.saturating_sub(Self::frame_cost(&frame));
        Some(frame)
    }

    /// Поток этого узла кончился (или переподписываемся): его кадры больше
    /// не показываем и ждём ключевой кадр нового потока.
    fn reset_from(&mut self, from: &str) {
        self.frames.retain(|f| f.from != from);
        self.bytes = self.frames.iter().map(Self::frame_cost).sum();
        self.resync = true;
    }
}

/// Кадр, опубликованный издателем (своим или пришедшим в feed). Один объект
/// на кадр делят broadcast и окно недавних кадров, поэтому «подписка + хвост
/// без дыр» — атомарно.
struct Published {
    seq: u32,
    keyframe: bool,
    data: Vec<u8>,
}

/// Окно недавних кадров: с него новый подписчик начинает показ.
struct Recent {
    frames: VecDeque<Arc<Published>>,
    bytes: usize,
    last_keyframe: Option<Arc<Published>>,
    next_seq: u32,
}

impl Recent {
    fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            last_keyframe: None,
            next_seq: 0,
        }
    }

    fn push(&mut self, keyframe: bool, data: Vec<u8>) -> Arc<Published> {
        self.next_seq = self.next_seq.wrapping_add(1);
        if self.next_seq == 0 {
            self.next_seq = 1;
        }
        let published = Arc::new(Published {
            seq: self.next_seq,
            keyframe,
            data,
        });
        if keyframe {
            self.last_keyframe = Some(published.clone());
        }
        self.bytes += published.data.len();
        self.frames.push_back(published.clone());
        while self.frames.len() > RECENT_FRAMES || self.bytes > RECENT_BYTES {
            let Some(old) = self.frames.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(old.data.len());
            if self.last_keyframe.as_ref().map(|kf| kf.seq) == Some(old.seq) {
                // ключевой выпал из окна — нового подписчика отправим
                // к следующему IDR
                self.last_keyframe = None;
            }
        }
        published
    }

    /// Хвост от последнего ключевого кадра и номер последнего кадра в нём.
    /// Без ключевого хвост пуст: P-кадры без опоры декодеру не нужны.
    fn handoff(&self) -> (Vec<Arc<Published>>, u32) {
        let mut tail = Vec::new();
        if let Some(keyframe) = self.last_keyframe.clone() {
            let after = keyframe.seq;
            tail.push(keyframe);
            tail.extend(
                self.frames
                    .iter()
                    .filter(|f| f.seq > after)
                    .map(|f| (*f).clone()),
            );
        }
        let last = tail.last().map(|f| f.seq).unwrap_or(self.next_seq);
        (tail, last)
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.last_keyframe = None;
    }
}

/// Поток издателя, который хост сейчас принимает и раздаёт зрителям.
struct FeedRelay {
    id: u64,
    publisher: String,
    gen: u32,
    stop: watch::Sender<bool>,
}

/// Окно кадров и relay под одним замком: подписка видит либо хвост своего
/// потока, либо уже новый — но не смесь двух.
struct Window {
    recent: Recent,
    relay: Option<FeedRelay>,
}

impl Window {
    fn new() -> Self {
        Self {
            recent: Recent::new(),
            relay: None,
        }
    }

    /// Свой кадр: поколение сверяется уже под замком. Иначе кадр, отправленный
    /// из GUI после `begin_stream`, мог бы попасть в окно новой трансляции.
    fn publish_own(
        &mut self,
        gen: u32,
        my_gen: u32,
        keyframe: bool,
        data: Vec<u8>,
    ) -> Option<Arc<Published>> {
        if gen == 0 || gen != my_gen {
            return None;
        }
        Some(self.recent.push(keyframe, data))
    }

    /// Кадр из feed: принимаем только от действующего relay и только пока
    /// сами не вещаем.
    fn publish_relay(
        &mut self,
        id: u64,
        my_gen: u32,
        keyframe: bool,
        data: Vec<u8>,
    ) -> Option<Arc<Published>> {
        if my_gen != 0 {
            return None;
        }
        if self.relay.as_ref().map(|r| r.id) != Some(id) {
            return None;
        }
        Some(self.recent.push(keyframe, data))
    }

    /// Снимает relay, если он всё ещё `id`: старая задача чтения не должна
    /// гасить новый feed. `true` — сняли именно его.
    fn release_relay(&mut self, id: u64) -> bool {
        if self.relay.as_ref().map(|r| r.id) == Some(id) {
            self.relay = None;
            self.recent.clear();
            true
        } else {
            false
        }
    }

    /// Снимает relay и очищает окно (своя трансляция начинается или кончается).
    fn take_relay(&mut self) -> Option<FeedRelay> {
        self.recent.clear();
        self.relay.take()
    }
}

/// Открытое видео-соединение к одному зрителю. Соединение живёт внутри
/// задачи отправки; `stop` её будит, и вместе с задачей освобождается
/// QUIC-соединение и его буферы.
struct Viewer {
    id: u64,
    stop: watch::Sender<bool>,
    alive: Arc<AtomicBool>,
}

/// На что смотрит зритель. Прямая подписка — к издателю, релей — к хосту,
/// который форвардит поток указанного издателя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubTarget {
    Direct { node: String, gen: u32 },
    Relay { host: String, gen: u32, publisher: String },
}

impl SubTarget {
    fn node(&self) -> &str {
        match self {
            SubTarget::Direct { node, .. } => node,
            SubTarget::Relay { host, .. } => host,
        }
    }

    fn gen(&self) -> u32 {
        match self {
            SubTarget::Direct { gen, .. } | SubTarget::Relay { gen, .. } => *gen,
        }
    }

    fn publisher(&self) -> Option<&str> {
        match self {
            SubTarget::Direct { .. } => None,
            SubTarget::Relay { publisher, .. } => Some(publisher),
        }
    }
}

/// Текущая подписка зрителя: ровно одна, при смене источника старую закрываем.
struct ActiveSub {
    id: u64,
    node: String,
    gen: u32,
    /// `None` — прямая подписка; `Some` — релей у хоста.
    publisher: Option<String>,
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct ViewerState {
    active: Option<ActiveSub>,
    /// Последняя неудача: (NodeID, поколение, когда). Повтор не раньше
    /// `RETRY_AFTER` и только для того же потока.
    last_fail: Option<(String, u32, Instant)>,
}

/// Исходящий feed издателя к хосту комнаты: ровно один.
struct ActiveFeed {
    id: u64,
    node: String,
    gen: u32,
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct FeedState {
    active: Option<ActiveFeed>,
    last_fail: Option<(String, u32, Instant)>,
}

/// Куда зрителю идти за картинкой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewRoute {
    /// Показывать нечего.
    None,
    /// Напрямую к издателю.
    Direct { publisher: String, gen: u32 },
    /// Через хост: он форвардит поток издателя.
    Relay {
        host: String,
        publisher: String,
        gen: u32,
    },
    /// Я сам хост: поток издателя приходит ко мне как feed.
    FeedWatch { publisher: String, gen: u32 },
}

impl ViewRoute {
    /// Подписка для `VideoLink::sync_viewing`. У хоста её нет: кадры
    /// приходят ему же в feed.
    pub fn target(&self) -> Option<SubTarget> {
        match self {
            ViewRoute::None | ViewRoute::FeedWatch { .. } => None,
            ViewRoute::Direct { publisher, gen } => Some(SubTarget::Direct {
                node: publisher.clone(),
                gen: *gen,
            }),
            ViewRoute::Relay {
                host,
                publisher,
                gen,
            } => Some(SubTarget::Relay {
                host: host.clone(),
                gen: *gen,
                publisher: publisher.clone(),
            }),
        }
    }

    /// Чьи кадры появятся в окне зрителя: имя источника сравнивается, чтобы
    /// окно не висело на прошлом.
    pub fn expected_peer(&self) -> Option<&str> {
        match self {
            ViewRoute::None => None,
            ViewRoute::Direct { publisher, .. } | ViewRoute::FeedWatch { publisher, .. } => {
                Some(publisher)
            }
            ViewRoute::Relay { host, .. } => Some(host),
        }
    }
}

/// Маршрут зрителя. `live` — анонсы потока в комнате (без своего), по
/// алфавиту NodeID; смотрим первый — так все зрители сходятся на одном.
pub fn choose_view_route(
    is_host: bool,
    left: bool,
    my_gen: u32,
    live: &[(String, u32)],
    host: Option<&str>,
    host_busy: bool,
) -> ViewRoute {
    if left || my_gen != 0 {
        return ViewRoute::None;
    }
    let Some((publisher, gen)) = live.first() else {
        return ViewRoute::None;
    };
    if is_host {
        // Хост смотрит тот же поток, что и раздаёт: свой feed.
        return ViewRoute::FeedWatch {
            publisher: publisher.clone(),
            gen: *gen,
        };
    }
    if let Some(host) = host {
        // Вещающий хост релеем быть не может: его окно занято своим потоком.
        if host != publisher && !host_busy {
            return ViewRoute::Relay {
                host: host.to_string(),
                publisher: publisher.clone(),
                gen: *gen,
            };
        }
    }
    ViewRoute::Direct {
        publisher: publisher.clone(),
        gen: *gen,
    }
}

/// Куда издателю отдавать feed. `None` — отдавать некому или незачем:
/// хост сам вещает, хоста нет, либо комната смотрит чужой поток (первый
/// по алфавиту), и мой туда всё равно никто не откроет.
pub fn choose_feed_target(
    is_host: bool,
    my_gen: u32,
    live: &[(String, u32)],
    host: Option<&str>,
    host_busy: bool,
) -> Option<(String, u32)> {
    if my_gen == 0 || is_host || host_busy || !live.is_empty() {
        return None;
    }
    host.map(|h| (h.to_string(), my_gen))
}

pub struct VideoLink {
    p2p: Arc<IrohP2pManager>,
    rt: tokio::runtime::Handle,
    /// Кто сейчас в эфире: сюда издатель кладёт номер своей трансляции,
    /// сюда же пишутся анонсы соседей из IrohSync.
    directory: Arc<VideoDirectory>,
    /// Хост ли я: только хост принимает feed издателя.
    is_host: Arc<AtomicBool>,
    /// После «Выйти из эфира» входящие видео-соединения не принимаем:
    /// `disconnect_all` их не закрывает.
    room: Arc<RoomManager>,
    /// Номер предыдущей трансляции: перезапуск в ту же секунду обязан дать
    /// другой gen, иначе зритель не заметит подмену.
    last_gen: AtomicU32,
    outgoing: broadcast::Sender<Arc<Published>>,
    window: std::sync::Mutex<Window>,
    remote: std::sync::Mutex<RemoteQueue>,
    /// Счётчик сбросов очереди зрителя: GUI по его смене выбрасывает окно,
    /// даже если имя источника не изменилось.
    remote_epoch: AtomicU64,
    buffer_ms: AtomicU32,
    viewing: std::sync::Mutex<ViewerState>,
    feeding: std::sync::Mutex<FeedState>,
    next_id: AtomicU64,
    viewers: std::sync::Mutex<Vec<Viewer>>,
}

impl VideoLink {
    pub fn spawn(
        p2p: Arc<IrohP2pManager>,
        rt: tokio::runtime::Handle,
        directory: Arc<VideoDirectory>,
        is_host: Arc<AtomicBool>,
        room: Arc<RoomManager>,
    ) -> Arc<Self> {
        let (outgoing, _) = broadcast::channel(BROADCAST_FRAMES);
        let link = Arc::new(Self {
            p2p,
            rt,
            directory,
            is_host,
            room,
            last_gen: AtomicU32::new(0),
            outgoing,
            window: std::sync::Mutex::new(Window::new()),
            remote: std::sync::Mutex::new(RemoteQueue::new()),
            remote_epoch: AtomicU64::new(1),
            buffer_ms: AtomicU32::new(DEFAULT_BUFFER_MS),
            viewing: std::sync::Mutex::new(ViewerState::default()),
            feeding: std::sync::Mutex::new(FeedState::default()),
            next_id: AtomicU64::new(0),
            viewers: std::sync::Mutex::new(Vec::new()),
        });
        let l = link.clone();
        link.rt.spawn(async move { l.accept_loop().await });
        link
    }

    /// Принимает входящие соединения: подписку зрителя, feed издателя или
    /// релей-подписку.
    async fn accept_loop(self: Arc<Self>) {
        while let Some(conn) = self.p2p.recv_video_conn().await {
            let me = self.clone();
            self.rt
                .spawn(async move { me.handle_control(conn).await });
        }
    }

    async fn handle_control(self: Arc<Self>, conn: Connection) {
        let from = conn.remote_id().to_string();
        let pair = timeout(SUBSCRIBE_TIMEOUT, async { conn.accept_bi().await }).await;
        let Ok(Ok((mut send, mut recv))) = pair else {
            // Соединение без запроса — ему тут не место.
            conn.close(0u32.into(), b"no-subscribe");
            return;
        };
        let request = timeout(SUBSCRIBE_TIMEOUT, recv.read_to_end(CONTROL_LIMIT)).await;
        let cmd = match request {
            Ok(Ok(data)) => parse_control(&data),
            _ => None,
        };

        let my_gen = self.directory.my_gen();
        let is_host = self.is_host.load(Ordering::Relaxed);
        let left = self.room.is_left();
        // Живой ли анонс издателя: feed от узла, которого нет в каталоге,
        // не принимаем — это может быть старый поток или чужой участник.
        let feed_live = matches!(
            cmd.as_ref(),
            Some(Control::Feed { gen }) if self.directory.is_live(&from, *gen, Instant::now())
        );
        let relay = self
            .window
            .lock()
            .ok()
            .and_then(|window| window.relay.as_ref().map(|r| (r.publisher.clone(), r.gen)));
        let action = match &cmd {
            Some(cmd) => decide_control(
                cmd,
                my_gen,
                is_host,
                left,
                feed_live,
                relay.as_ref().map(|(publisher, gen)| (publisher.as_str(), *gen)),
            ),
            None => Action::Reject,
        };

        // Feed регистрируется до подтверждения: иначе первые кадры пришли бы
        // раньше, чем окно узнает о новом relay.
        let mut feed_task = None;
        let accepted = match &action {
            Action::Reject => false,
            Action::Serve => true,
            Action::Feed { gen } => {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let (stop, stop_rx) = watch::channel(false);
                match self.window.lock() {
                    Ok(mut window) => {
                        if self.directory.my_gen() != 0
                            || !self.is_host.load(Ordering::Relaxed)
                        {
                            false
                        } else {
                            let old = window.relay.replace(FeedRelay {
                                id,
                                publisher: from.clone(),
                                gen: *gen,
                                stop,
                            });
                            window.recent.clear();
                            if let Some(old) = old {
                                let _ = old.stop.send(true);
                            }
                            feed_task = Some((id, stop_rx));
                            true
                        }
                    }
                    Err(_) => false,
                }
            }
        };

        let ack = encode_ack(if accepted { 0 } else { 1 }, my_gen);
        let _ = send.write_all(&ack).await;
        let _ = send.finish();

        if !accepted {
            println!(
                "[ВИДЕО] Запрос от {} отклонён ({:?}, в эфире {})",
                from, cmd, my_gen
            );
            conn.close(0u32.into(), b"rejected");
            return;
        }
        match (action, feed_task) {
            (Action::Feed { gen }, Some((id, stop_rx))) => {
                println!("[ВИДЕО] Поток от {} принят в релей (поколение {gen})", from);
                let me = self.clone();
                self.rt
                    .spawn(async move { me.read_feed(id, from, conn, stop_rx).await });
            }
            _ => {
                println!("[ВИДЕО] Зритель подписался: {}", from);
                self.start_sender(conn, from);
            }
        }
    }

    /// Регистрирует зрителя и запускает задачу отправки кадров на этом
    /// соединении.
    fn start_sender(self: &Arc<Self>, conn: Connection, node: String) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (stop, stop_rx) = watch::channel(false);
        let alive = Arc::new(AtomicBool::new(true));
        if let Ok(mut viewers) = self.viewers.lock() {
            viewers.push(Viewer {
                id,
                stop,
                alive: alive.clone(),
            });
        }
        let me = self.clone();
        self.rt.spawn(async move {
            me.send_frames(conn, stop_rx, None).await;
            alive.store(false, Ordering::Relaxed);
            if let Ok(mut viewers) = me.viewers.lock() {
                viewers.retain(|v| v.id != id);
            }
            println!("[ВИДЕО] Зритель {} отключился", node);
        });
    }

    /// Хост читает feed издателя: те же байты кладёт в окно и раздаёт
    /// зрителям, а себе — в очередь показа.
    async fn read_feed(
        self: Arc<Self>,
        id: u64,
        publisher: String,
        conn: Connection,
        mut stop: watch::Receiver<bool>,
    ) {
        loop {
            let mut stream = tokio::select! {
                changed = stop.changed() => {
                    let _ = changed;
                    break;
                }
                _ = conn.closed() => break,
                next = conn.accept_uni() => match next {
                    Ok(stream) => stream,
                    Err(_) => break,
                },
            };
            let data = tokio::select! {
                changed = stop.changed() => {
                    let _ = changed;
                    break;
                }
                read = stream.read_to_end(MAX_FRAME) => match read {
                    Ok(data) => data,
                    Err(_) => continue,
                },
            };
            // Заголовок разбираем без копии payload: релею нужен только
            // признак ключевого кадра. Копия — лишь для своего окна показа.
            let Some(header) = frame_header(&data) else {
                eprintln!("[ВИДЕО] Битый кадр в потоке от {}", publisher);
                continue;
            };
            if let Some(frame) = parse_frame(&data, &publisher) {
                self.push_remote(frame);
            }
            let published = self
                .window
                .lock()
                .ok()
                .and_then(|mut window| {
                    window.publish_relay(id, self.directory.my_gen(), header.keyframe, data)
                });
            let Some(published) = published else {
                // Relay сняли (своя трансляция или новый feed) — дальше не читаем.
                break;
            };
            let _ = self.outgoing.send(published);
        }

        // Снимаем relay, только если он всё ещё наш: старая задача не должна
        // гасить новый feed.
        let removed = self
            .window
            .lock()
            .map(|mut window| window.release_relay(id))
            .unwrap_or(false);
        if removed {
            self.stop_viewers();
            self.clear_remote();
            println!("[ВИДЕО] Поток от {} через релей окончен", publisher);
        }
        conn.close(0u32.into(), b"feed ended");
    }

    /// Подписывается на broadcast и снимает окно недавних кадров под одним
    /// мьютексом с публикацией: хвост и последующие кадры стыкуются без дыр.
    fn handoff(&self) -> (broadcast::Receiver<Arc<Published>>, Vec<Arc<Published>>, u32) {
        match self.window.lock() {
            Ok(window) => {
                let rx = self.outgoing.subscribe();
                let (tail, last) = window.recent.handoff();
                (rx, tail, last)
            }
            Err(_) => (self.outgoing.subscribe(), Vec::new(), 0),
        }
    }

    /// Шлёт кадры одному зрителю (или хосту, если это feed). `required_gen`
    /// — поколение, в котором обязан оставаться издатель: после перезапуска
    /// поток прекращается сам.
    async fn send_frames(
        self: &Arc<Self>,
        conn: Connection,
        mut stop: watch::Receiver<bool>,
        required_gen: Option<u32>,
    ) {
        let (mut rx, tail, mut last_seq) = self.handoff();
        // Хвост всегда начинается с ключевого. Если ключевого в окне уже нет,
        // P-кадры из broadcast не шлём: без опоры декодер нарисует мусор —
        // ждём следующий IDR.
        let mut need_key = tail.is_empty();
        for frame in &tail {
            if send_published(&conn, frame).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return;
                    }
                }
                _ = conn.closed() => return,
                item = rx.recv() => match item {
                    Ok(frame) => {
                        if let Some(gen) = required_gen {
                            if self.directory.my_gen() != gen {
                                return;
                            }
                        }
                        // Хвост и broadcast пересекаются: кадры, уже ушедшие
                        // в хвосте, повторно не шлём.
                        if frame.seq <= last_seq {
                            continue;
                        }
                        if need_key && !frame.keyframe {
                            continue;
                        }
                        need_key = false;
                        if send_published(&conn, &frame).await.is_err() {
                            return;
                        }
                        last_seq = frame.seq;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(gen) = required_gen {
                            if self.directory.my_gen() != gen {
                                return;
                            }
                        }
                        // Не успели записать 240 кадров — берём хвост заново,
                        // с ключевого, а не продолжаем с P-кадра.
                        let (new_rx, tail, last) = self.handoff();
                        rx = new_rx;
                        last_seq = last;
                        need_key = tail.is_empty();
                        for frame in &tail {
                            if send_published(&conn, frame).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }

    /// Начинает новую трансляцию. Номер ненулевой и не равен предыдущему —
    /// по нему зрители отличают перезапуск. Relay и feed при этом снимаются:
    /// в одном окне своя картинка и чужой поток не живут.
    pub fn begin_stream(&self) -> u32 {
        let mut gen = self.directory.allocate_gen();
        if self.last_gen.swap(gen, Ordering::SeqCst) == gen {
            gen = gen.wrapping_add(1);
            if gen == 0 {
                gen = 1;
            }
            self.last_gen.store(gen, Ordering::SeqCst);
        }
        self.directory.set_my_gen(gen);
        self.drop_relay();
        self.stop_feed();
        self.stop_viewers();
        self.clear_remote();
        gen
    }

    /// Останавливает трансляцию: анонс gen 0, зрители отключаются.
    pub fn end_stream(&self) {
        self.directory.set_my_gen(0);
        self.drop_relay();
        self.stop_feed();
        self.stop_viewers();
        self.clear_remote();
    }

    /// Снимает relay под замком окна и будит задачу его чтения.
    fn drop_relay(&self) {
        let relay = self.window.lock().ok().and_then(|mut window| window.take_relay());
        if let Some(relay) = relay {
            let _ = relay.stop.send(true);
        }
    }

    /// Закрывает все видео-соединения: трансляция остановлена. Без этого
    /// каждый следующий старт добавлял по соединению на зрителя, а старые
    /// оставались висеть и держали память.
    pub fn stop_viewers(&self) {
        if let Ok(mut viewers) = self.viewers.lock() {
            for v in viewers.drain(..) {
                let _ = v.stop.send(true);
            }
        }
    }

    /// Сколько зрителей сейчас смотрят нашу трансляцию. Раньше в карточке
    /// показывали размер комнаты — «зрит.» было неправдой, если стрим
    /// никто не открывал или смотрел не участник комнаты.
    pub fn viewer_count(&self) -> usize {
        match self.viewers.lock() {
            Ok(viewers) => viewers.iter().filter(|v| v.alive.load(Ordering::Relaxed)).count(),
            Err(_) => 0,
        }
    }

    /// Публикует свой кадр: нумерует, кладёт в окно и рассылает подписчикам —
    /// всё под одним замком, чтобы подписка получала цепочку без дыр.
    /// Окно ведётся всегда: иначе вошедшему зрителю пришлось бы ждать
    /// следующего IDR.
    pub fn publish(&self, unit: &EncodedUnit, info: &StreamInfo) {
        let gen = self.directory.my_gen();
        if gen == 0 {
            return;
        }
        let data = encode_frame(unit, info);
        let Ok(mut window) = self.window.lock() else {
            return;
        };
        if let Some(frame) =
            window.publish_own(gen, self.directory.my_gen(), unit.keyframe, data)
        {
            let _ = self.outgoing.send(frame);
        }
    }

    pub fn set_buffer_ms(&self, ms: u32) {
        self.buffer_ms.store(ms.min(5000), Ordering::Relaxed);
    }

    pub fn buffer_ms(&self) -> u32 {
        self.buffer_ms.load(Ordering::Relaxed)
    }

    /// Счётчик сбросов окна зрителя: сменился — прошлую картинку не показывать.
    pub fn remote_epoch(&self) -> u64 {
        self.remote_epoch.load(Ordering::Relaxed)
    }

    /// Подстраивает подписку под выбранный маршрут. Вызывается каждый кадр:
    /// активная подписка на тот же поток не трогается.
    pub fn sync_viewing(self: &Arc<Self>, target: Option<SubTarget>) {
        let Ok(mut state) = self.viewing.lock() else {
            return;
        };
        if let Some(active) = state.active.as_ref() {
            let same = target
                .as_ref()
                .map(|t| {
                    t.node() == active.node
                        && t.gen() == active.gen
                        && t.publisher() == active.publisher.as_deref()
                })
                .unwrap_or(false);
            if same {
                return;
            }
        } else if target.is_none() {
            state.last_fail = None;
            return;
        }

        let previous = state.active.take();
        let Some(target) = target else {
            if let Some(old) = previous {
                let _ = old.stop.send(true);
            }
            state.last_fail = None;
            return;
        };

        let node = target.node().to_string();
        let gen = target.gen();
        let publisher = target.publisher().map(str::to_string);
        let throttled = state
            .last_fail
            .as_ref()
            .map(|(fail_node, fail_gen, at)| {
                *fail_node == node && *fail_gen == gen && at.elapsed() < RETRY_AFTER
            })
            .unwrap_or(false);
        if let Some(old) = previous {
            let _ = old.stop.send(true);
        }
        if throttled {
            return;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (stop, stop_rx) = watch::channel(false);
        state.active = Some(ActiveSub {
            id,
            node,
            gen,
            publisher,
            stop,
        });
        drop(state);

        let me = self.clone();
        self.rt.spawn(async move {
            me.run_subscription(id, target, stop_rx).await;
        });
    }

    /// Снимает подписку (вышли из комнаты). Очередь показывать больше нечего.
    pub fn unsubscribe(&self) {
        if let Ok(mut state) = self.viewing.lock() {
            if let Some(active) = state.active.take() {
                let _ = active.stop.send(true);
            }
            state.last_fail = None;
        }
        self.clear_remote();
    }

    /// Подстраивает исходящий feed к хосту комнаты. Как и показ, он привязан
    /// к паре (хост, своё поколение): перезапуск трансляции — новый feed.
    pub fn sync_feed(self: &Arc<Self>, target: Option<(String, u32)>) {
        let Ok(mut state) = self.feeding.lock() else {
            return;
        };
        if let Some(active) = state.active.as_ref() {
            let same = target
                .as_ref()
                .map(|(node, gen)| *node == active.node && *gen == active.gen)
                .unwrap_or(false);
            if same {
                return;
            }
        } else if target.is_none() {
            state.last_fail = None;
            return;
        }

        let previous = state.active.take();
        let Some((node, gen)) = target else {
            if let Some(old) = previous {
                let _ = old.stop.send(true);
            }
            state.last_fail = None;
            return;
        };

        let throttled = state
            .last_fail
            .as_ref()
            .map(|(fail_node, fail_gen, at)| {
                *fail_node == node && *fail_gen == gen && at.elapsed() < RETRY_AFTER
            })
            .unwrap_or(false);
        if let Some(old) = previous {
            let _ = old.stop.send(true);
        }
        if throttled {
            return;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (stop, stop_rx) = watch::channel(false);
        state.active = Some(ActiveFeed {
            id,
            node: node.clone(),
            gen,
            stop,
        });
        drop(state);

        let me = self.clone();
        self.rt.spawn(async move {
            me.run_feed(id, node, gen, stop_rx).await;
        });
    }

    /// Снимает активный feed и будит его задачу.
    pub fn stop_feed(&self) {
        if let Ok(mut state) = self.feeding.lock() {
            if let Some(active) = state.active.take() {
                let _ = active.stop.send(true);
            }
            state.last_fail = None;
        }
    }

    fn clear_remote(&self) {
        if let Ok(mut q) = self.remote.lock() {
            q.frames.clear();
            q.bytes = 0;
            q.resync = true;
        }
        self.remote_epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// Кадры потока `node` больше не относятся к новой подписке.
    fn reset_remote_from(&self, node: &str) {
        if let Ok(mut q) = self.remote.lock() {
            q.reset_from(node);
        }
        self.remote_epoch.fetch_add(1, Ordering::Relaxed);
    }

    async fn run_subscription(
        self: Arc<Self>,
        id: u64,
        target: SubTarget,
        mut stop: watch::Receiver<bool>,
    ) {
        let node = target.node().to_string();
        let gen = target.gen();
        let received = self.connect_and_read(&target, &mut stop).await;
        let stopped = *stop.borrow();
        if let Ok(mut state) = self.viewing.lock() {
            if state.active.as_ref().map(|a| a.id) == Some(id) {
                state.active = None;
            }
            if !stopped {
                state.last_fail = Some((node.clone(), gen, Instant::now()));
            }
        }
        if stopped {
            return;
        }
        if received {
            println!("[ВИДЕО] Поток от {} окончен", node);
        }
    }

    /// Открывает соединение, отправляет подписку, ждёт подтверждения и читает
    /// кадры uni-стримами. `true` — поток шёл и кончился сам; `false` —
    /// подключиться не удалось.
    async fn connect_and_read(
        &self,
        target: &SubTarget,
        stop: &mut watch::Receiver<bool>,
    ) -> bool {
        let node = target.node();
        let gen = target.gen();
        let Ok(target_id) = node.parse::<iroh::EndpointId>() else {
            return false;
        };
        let conn = match timeout(
            CONNECT_TIMEOUT,
            self.p2p.endpoint().connect(target_id, VIDEO_ALPN),
        )
        .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                eprintln!("[ВИДЕО] Не удалось подключиться к {node}: {e}");
                return false;
            }
            Err(_) => {
                eprintln!("[ВИДЕО] Таймаут подключения к {node}");
                return false;
            }
        };

        let pair = timeout(SUBSCRIBE_TIMEOUT, async { conn.open_bi().await }).await;
        let Ok(Ok((mut send, mut recv))) = pair else {
            conn.close(0u32.into(), b"no-control");
            return false;
        };
        let request = match target {
            SubTarget::Direct { gen, .. } => encode_subscribe(*gen).to_vec(),
            SubTarget::Relay { gen, publisher, .. } => {
                encode_relay_subscribe(*gen, publisher)
            }
        };
        if send.write_all(&request).await.is_err() {
            return false;
        }
        let _ = send.finish();

        let ack = timeout(SUBSCRIBE_TIMEOUT, recv.read_to_end(CONTROL_LIMIT)).await;
        let accepted = match ack {
            Ok(Ok(data)) => parse_ack(&data).map(|(status, _)| status == 0),
            _ => None,
        };
        if accepted != Some(true) {
            println!(
                "[ВИДЕО] {} не принял подписку (поколение {gen})",
                node
            );
            conn.close(0u32.into(), b"rejected");
            return false;
        }
        println!("[ВИДЕО] Подписка принята: {} (поколение {gen})", node);
        // Кадры прошлой подписки к новому потоку не относятся.
        self.reset_remote_from(node);

        loop {
            let mut stream = tokio::select! {
                changed = stop.changed() => {
                    let _ = changed;
                    return false;
                }
                next = async { conn.accept_uni().await } => match next {
                    Ok(stream) => stream,
                    Err(_) => break,
                },
            };
            let data = tokio::select! {
                changed = stop.changed() => {
                    let _ = changed;
                    return false;
                }
                read = stream.read_to_end(MAX_FRAME) => match read {
                    Ok(data) => data,
                    Err(_) => continue,
                },
            };
            match parse_frame(&data, node) {
                Some(frame) => self.push_remote(frame),
                None => eprintln!("[ВИДЕО] Битый кадр от {}", node),
            }
        }

        // Трансляция кончилась: недоигранные кадры этого узла держать незачем.
        self.reset_remote_from(node);
        conn.close(0u32.into(), b"stream ended");
        true
    }

    async fn run_feed(
        self: Arc<Self>,
        id: u64,
        node: String,
        gen: u32,
        mut stop: watch::Receiver<bool>,
    ) {
        let ran = self.connect_and_feed(&node, gen, &mut stop).await;
        let stopped = *stop.borrow();
        if let Ok(mut state) = self.feeding.lock() {
            if state.active.as_ref().map(|a| a.id) == Some(id) {
                state.active = None;
            }
            if !stopped {
                state.last_fail = Some((node.clone(), gen, Instant::now()));
            }
        }
        if stopped {
            return;
        }
        if ran {
            println!("[ВИДЕО] Поток хосту {} окончен", node);
        }
    }

    /// Отдаёт хост свой поток. `true` — поток шёл и кончился сам; `false` —
    /// хост не принял или недоступен.
    async fn connect_and_feed(
        self: &Arc<Self>,
        node: &str,
        gen: u32,
        stop: &mut watch::Receiver<bool>,
    ) -> bool {
        let Ok(target) = node.parse::<iroh::EndpointId>() else {
            return false;
        };
        let conn = match timeout(
            CONNECT_TIMEOUT,
            self.p2p.endpoint().connect(target, VIDEO_ALPN),
        )
        .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                eprintln!("[ВИДЕО] Не удалось подключиться к хосту {node}: {e}");
                return false;
            }
            Err(_) => {
                eprintln!("[ВИДЕО] Таймаут подключения к хосту {node}");
                return false;
            }
        };

        let pair = timeout(SUBSCRIBE_TIMEOUT, async { conn.open_bi().await }).await;
        let Ok(Ok((mut send, mut recv))) = pair else {
            conn.close(0u32.into(), b"no-control");
            return false;
        };
        if send.write_all(&encode_feed(gen)).await.is_err() {
            return false;
        }
        let _ = send.finish();

        let ack = timeout(SUBSCRIBE_TIMEOUT, recv.read_to_end(CONTROL_LIMIT)).await;
        let accepted = match ack {
            Ok(Ok(data)) => parse_ack(&data).map(|(status, _)| status == 0),
            _ => None,
        };
        if accepted != Some(true) {
            println!("[ВИДЕО] Хост {} не принял поток (поколение {gen})", node);
            conn.close(0u32.into(), b"rejected");
            return false;
        }
        println!("[ВИДЕО] Поток хосту {} отдан (поколение {gen})", node);
        self.send_frames(conn, stop.clone(), Some(gen)).await;
        true
    }

    fn push_remote(&self, frame: RemoteFrame) {
        let buffer = Duration::from_millis(self.buffer_ms.load(Ordering::Relaxed) as u64);
        if let Ok(mut q) = self.remote.lock() {
            q.push(frame, buffer);
        }
    }

    /// Кадр, чей буфер показа уже истёк (для GUI, без блокировки).
    pub fn try_recv_due(&self) -> Option<RemoteFrame> {
        let buffer = Duration::from_millis(self.buffer_ms.load(Ordering::Relaxed) as u64);
        let now = Instant::now();
        self.remote.lock().ok()?.pop_due(buffer, now)
    }

    /// Есть ли вообще что показывать — по состоянию очереди и подписки.
    pub fn has_remote(&self) -> bool {
        self.remote
            .lock()
            .map(|q| !q.frames.is_empty())
            .unwrap_or(false)
    }
}

async fn send_published(conn: &Connection, frame: &Published) -> Result<(), ()> {
    let mut send = conn.open_uni().await.map_err(|_| ())?;
    send.write_all(&frame.data).await.map_err(|_| ())?;
    let _ = send.finish();
    Ok(())
}

/// Пришедший управляющий запрос.
#[derive(Debug, PartialEq, Eq)]
enum Control {
    Subscribe { gen: u32 },
    Feed { gen: u32 },
    Relay { gen: u32, publisher: String },
}

/// Что делать с принятым запросом.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Обслуживать зрителя из окна (подписка к издателю или к релею хоста).
    Serve,
    /// Принять поток издателя в релей.
    Feed { gen: u32 },
    Reject,
}

/// Чистое решение по запросу: без сети и без состояния, чтобы проверялось
/// тестами.
fn decide_control(
    cmd: &Control,
    my_gen: u32,
    is_host: bool,
    left: bool,
    feed_live: bool,
    relay: Option<(&str, u32)>,
) -> Action {
    match cmd {
        Control::Subscribe { gen } => {
            if my_gen != 0 && *gen == my_gen {
                Action::Serve
            } else {
                Action::Reject
            }
        }
        Control::Feed { gen } => {
            if is_host && !left && my_gen == 0 && *gen != 0 && feed_live {
                Action::Feed { gen: *gen }
            } else {
                Action::Reject
            }
        }
        Control::Relay { gen, publisher } => {
            // Поколение — секунды, у двух издателей они совпадают: отдаём
            // поток только тому издателю, который сейчас в релее.
            if !left && relay == Some((publisher.as_str(), *gen)) {
                Action::Serve
            } else {
                Action::Reject
            }
        }
    }
}

fn encode_subscribe(gen: u32) -> [u8; 5] {
    let mut out = [0u8; 5];
    out[0] = KIND_SUBSCRIBE;
    out[1..].copy_from_slice(&gen.to_le_bytes());
    out
}

fn encode_feed(gen: u32) -> [u8; 5] {
    let mut out = [0u8; 5];
    out[0] = KIND_FEED;
    out[1..].copy_from_slice(&gen.to_le_bytes());
    out
}

fn encode_relay_subscribe(gen: u32, publisher: &str) -> Vec<u8> {
    let bytes = publisher.as_bytes();
    let mut out = Vec::with_capacity(7 + bytes.len());
    out.push(KIND_RELAY);
    out.extend_from_slice(&gen.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

fn parse_control(data: &[u8]) -> Option<Control> {
    let kind = *data.first()?;
    match kind {
        KIND_SUBSCRIBE => {
            if data.len() < 5 {
                return None;
            }
            Some(Control::Subscribe {
                gen: u32::from_le_bytes(data[1..5].try_into().ok()?),
            })
        }
        KIND_FEED => {
            if data.len() < 5 {
                return None;
            }
            Some(Control::Feed {
                gen: u32::from_le_bytes(data[1..5].try_into().ok()?),
            })
        }
        KIND_RELAY => {
            if data.len() < 7 {
                return None;
            }
            let gen = u32::from_le_bytes(data[1..5].try_into().ok()?);
            let len = u16::from_le_bytes(data[5..7].try_into().ok()?) as usize;
            if len != NODE_ID_LEN || data.len() < 7 + len {
                return None;
            }
            let publisher = std::str::from_utf8(&data[7..7 + len]).ok()?;
            if !publisher.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            Some(Control::Relay {
                gen,
                publisher: publisher.to_string(),
            })
        }
        _ => None,
    }
}

fn encode_ack(status: u8, gen: u32) -> [u8; 6] {
    let mut out = [0u8; 6];
    out[0] = KIND_ACK;
    out[1] = status;
    out[2..].copy_from_slice(&gen.to_le_bytes());
    out
}

fn parse_ack(data: &[u8]) -> Option<(u8, u32)> {
    if data.len() < 6 || data[0] != KIND_ACK {
        return None;
    }
    Some((data[1], u32::from_le_bytes(data[2..6].try_into().ok()?)))
}

/// Заголовок кадра без копирования payload.
struct FrameHeader {
    keyframe: bool,
    seq_len: usize,
    width: u32,
    height: u32,
    fps: u32,
    pts_100ns: i64,
    len: usize,
}

fn frame_header(data: &[u8]) -> Option<FrameHeader> {
    if data.len() < 28 || data[0] != KIND_FRAME {
        return None;
    }
    let flags = data[1];
    let seq_len = u16::from_le_bytes(data[2..4].try_into().ok()?) as usize;
    let width = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let fps = u32::from_le_bytes(data[12..16].try_into().ok()?);
    let pts = i64::from_le_bytes(data[16..24].try_into().ok()?);
    let len = u32::from_le_bytes(data[24..28].try_into().ok()?) as usize;
    if data.len() < 28 + seq_len + len {
        return None;
    }
    Some(FrameHeader {
        keyframe: flags & 1 != 0,
        seq_len,
        width,
        height,
        fps,
        pts_100ns: pts,
        len,
    })
}

fn encode_frame(unit: &EncodedUnit, info: &StreamInfo) -> Vec<u8> {
    let has_seq = unit.keyframe && !info.sequence_header.is_empty();
    let seq: &[u8] = if has_seq {
        &info.sequence_header
    } else {
        &[]
    };
    let mut flags = 0u8;
    if unit.keyframe {
        flags |= 1;
    }
    if has_seq {
        flags |= 2;
    }

    let mut v = Vec::with_capacity(28 + seq.len() + unit.data.len());
    v.push(KIND_FRAME);
    v.push(flags);
    v.extend_from_slice(&(seq.len() as u16).to_le_bytes());
    v.extend_from_slice(&info.width.to_le_bytes());
    v.extend_from_slice(&info.height.to_le_bytes());
    v.extend_from_slice(&info.fps.to_le_bytes());
    v.extend_from_slice(&unit.pts_100ns.to_le_bytes());
    v.extend_from_slice(&(unit.data.len() as u32).to_le_bytes());
    v.extend_from_slice(seq);
    v.extend_from_slice(&unit.data);
    v
}

fn parse_frame(data: &[u8], from: &str) -> Option<RemoteFrame> {
    let h = frame_header(data)?;
    let rest = &data[28..];
    Some(RemoteFrame {
        from: from.to_string(),
        width: h.width,
        height: h.height,
        fps: h.fps,
        sequence_header: rest[..h.seq_len].to_vec(),
        unit: EncodedUnit {
            data: rest[h.seq_len..h.seq_len + h.len].to_vec(),
            keyframe: h.keyframe,
            pts_100ns: h.pts_100ns,
        },
        arrived: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(keyframe: bool, pts: i64, size: usize) -> EncodedUnit {
        EncodedUnit {
            data: vec![0xAB; size],
            keyframe,
            pts_100ns: pts,
        }
    }

    fn info() -> StreamInfo {
        StreamInfo {
            width: 1920,
            height: 1080,
            fps: 60,
            sequence_header: vec![0x67, 0x42],
        }
    }

    fn frame(from: &str, keyframe: bool, pts: i64, arrived: Instant) -> RemoteFrame {
        RemoteFrame {
            from: from.to_string(),
            width: 1920,
            height: 1080,
            fps: 60,
            sequence_header: vec![],
            unit: EncodedUnit {
                data: vec![1; 16],
                keyframe,
                pts_100ns: pts,
            },
            arrived,
        }
    }

    fn node_id(seed: char) -> String {
        std::iter::repeat(seed).take(NODE_ID_LEN).collect()
    }

    #[test]
    fn subscribe_roundtrip_and_garbage() {
        let bytes = encode_subscribe(0xDEAD_BEEF);
        assert_eq!(bytes[0], KIND_SUBSCRIBE);
        assert_eq!(
            parse_control(&bytes),
            Some(Control::Subscribe { gen: 0xDEAD_BEEF })
        );
        assert_eq!(parse_control(&bytes[..4]), None);
        // ответ издателя жалобой не считается
        assert_eq!(parse_control(&encode_ack(0, 7)), None);
        assert_eq!(parse_control(&[]), None);
    }

    #[test]
    fn feed_roundtrip_and_garbage() {
        let bytes = encode_feed(42);
        assert_eq!(bytes[0], KIND_FEED);
        assert_eq!(parse_control(&bytes), Some(Control::Feed { gen: 42 }));
        assert_eq!(parse_control(&bytes[..4]), None);
        // подписка и feed не путаются
        assert_ne!(
            parse_control(&encode_subscribe(42)),
            parse_control(&encode_feed(42))
        );
    }

    #[test]
    fn relay_roundtrip_and_garbage() {
        let publisher = node_id('a');
        let bytes = encode_relay_subscribe(7, &publisher);
        assert_eq!(bytes[0], KIND_RELAY);
        assert_eq!(
            parse_control(&bytes),
            Some(Control::Relay {
                gen: 7,
                publisher: publisher.clone()
            })
        );
        assert_eq!(parse_control(&bytes[..6]), None);
        // длина NodeID строго 64 символа hex
        assert_eq!(parse_control(&encode_relay_subscribe(7, "abc")), None);
        assert_eq!(
            parse_control(&encode_relay_subscribe(7, &node_id('z'))),
            None
        );
        // обрезанный NodeID
        let mut cut = bytes.clone();
        cut.pop();
        assert_eq!(parse_control(&cut), None);
    }

    #[test]
    fn ack_roundtrip_and_garbage() {
        let bytes = encode_ack(1, 0x0102_0304);
        assert_eq!(bytes[0], KIND_ACK);
        assert_eq!(parse_ack(&bytes), Some((1, 0x0102_0304)));
        assert_eq!(parse_ack(&bytes[..5]), None);
        assert_eq!(parse_ack(&encode_subscribe(1)), None);
        assert_eq!(parse_ack(&[]), None);
    }

    #[test]
    fn decide_accepts_own_generation_only() {
        let cmd = Control::Subscribe { gen: 5 };
        assert_eq!(decide_control(&cmd, 5, false, false, false, None), Action::Serve);
        assert_eq!(decide_control(&cmd, 6, false, false, false, None), Action::Reject);
        // не в эфире — подписке нечего отдать
        assert_eq!(decide_control(&cmd, 0, false, false, false, None), Action::Reject);
    }

    #[test]
    fn decide_feed_requires_host_and_free_window() {
        let cmd = Control::Feed { gen: 9 };
        assert_eq!(
            decide_control(&cmd, 0, true, false, true, None),
            Action::Feed { gen: 9 }
        );
        // не хост
        assert_eq!(decide_control(&cmd, 0, false, false, true, None), Action::Reject);
        // вышли из эфира
        assert_eq!(decide_control(&cmd, 0, true, true, true, None), Action::Reject);
        // сами вещаем
        assert_eq!(decide_control(&cmd, 3, true, false, true, None), Action::Reject);
        // издателя нет в каталоге
        assert_eq!(decide_control(&cmd, 0, true, false, false, None), Action::Reject);
        assert_eq!(decide_control(&Control::Feed { gen: 0 }, 0, true, false, true, None), Action::Reject);
    }

    #[test]
    fn decide_relay_matches_publisher_and_gen() {
        let publisher = node_id('a');
        let cmd = Control::Relay {
            gen: 4,
            publisher: publisher.clone(),
        };
        let current = Some((publisher.as_str(), 4u32));
        assert_eq!(decide_control(&cmd, 0, true, false, false, current), Action::Serve);
        // то же поколение (секунды совпадают), но другой издатель
        let other = node_id('b');
        let current = Some((other.as_str(), 4u32));
        assert_eq!(decide_control(&cmd, 0, true, false, false, current), Action::Reject);
        // другое поколение
        let current = Some((publisher.as_str(), 5u32));
        assert_eq!(decide_control(&cmd, 0, true, false, false, current), Action::Reject);
        // релея нет
        assert_eq!(decide_control(&cmd, 0, true, false, false, None), Action::Reject);
        // вышли из эфира
        let current = Some((publisher.as_str(), 4u32));
        assert_eq!(decide_control(&cmd, 0, true, true, false, current), Action::Reject);
    }

    #[test]
    fn frame_header_matches_full_parse() {
        let unit = unit(true, 42, 9);
        let info = info();
        let bytes = encode_frame(&unit, &info);
        let header = frame_header(&bytes).unwrap();
        assert!(header.keyframe);
        assert_eq!(header.seq_len, 2);
        assert_eq!((header.width, header.height, header.fps), (1920, 1080, 60));
        assert_eq!(header.pts_100ns, 42);
        assert_eq!(header.len, 9);
        // payload не копируется: заголовок на том же буфере
        let published = bytes.clone();
        assert!(frame_header(&published).is_some());

        assert!(frame_header(&[0; 27]).is_none());
        assert!(frame_header(&bytes[..30]).is_none());
        let mut foreign = bytes.clone();
        foreign[0] = KIND_SUBSCRIBE;
        assert!(frame_header(&foreign).is_none());
    }

    #[test]
    fn frame_roundtrip_keeps_wire_format() {
        let unit = unit(true, 42, 9);
        let info = info();
        let bytes = encode_frame(&unit, &info);
        assert_eq!(bytes[0], 1);
        assert_eq!(bytes[1], 3); // keyframe + SPS/PPS
        assert_eq!(u16::from_le_bytes(bytes[2..4].try_into().unwrap()), 2);
        assert_eq!(&bytes[28..30], &[0x67, 0x42]);

        let parsed = parse_frame(&bytes, "node-a").unwrap();
        assert_eq!(parsed.from, "node-a");
        assert_eq!((parsed.width, parsed.height, parsed.fps), (1920, 1080, 60));
        assert_eq!(parsed.sequence_header, info.sequence_header);
        assert!(parsed.unit.keyframe);
        assert_eq!(parsed.unit.pts_100ns, 42);
        assert_eq!(parsed.unit.data.len(), 9);
    }

    #[test]
    fn short_or_foreign_frames_rejected() {
        assert!(parse_frame(&[0; 27], "n").is_none());
        let mut bytes = encode_frame(&unit(false, 0, 1), &info());
        bytes[0] = KIND_SUBSCRIBE;
        assert!(parse_frame(&bytes, "n").is_none());
        let bytes = encode_frame(&unit(false, 0, 4), &info());
        assert!(parse_frame(&bytes[..30], "n").is_none());
    }

    fn relay(id: u64, publisher: &str, gen: u32) -> FeedRelay {
        let (stop, _rx) = watch::channel(false);
        FeedRelay {
            id,
            publisher: publisher.to_string(),
            gen,
            stop,
        }
    }

    #[test]
    fn window_gates_own_frames_by_generation() {
        let mut window = Window::new();
        // поколение сменилось между публикацией и замком
        assert!(window.publish_own(5, 6, true, vec![1]).is_none());
        assert!(window.publish_own(0, 0, true, vec![1]).is_none());
        assert!(window.recent.frames.is_empty());
        let frame = window.publish_own(5, 5, true, vec![1]).unwrap();
        assert_eq!(frame.seq, 1);
        assert!(window.recent.last_keyframe.is_some());
    }

    #[test]
    fn window_gates_relay_by_id_and_own_stream() {
        let mut window = Window::new();
        window.relay = Some(relay(7, "node-a", 3));
        // чужой id
        assert!(window.publish_relay(8, 0, true, vec![1]).is_none());
        // своя трансляция вытесняет relay
        assert!(window.publish_relay(7, 4, true, vec![1]).is_none());
        let frame = window.publish_relay(7, 0, true, vec![1]).unwrap();
        assert_eq!(frame.seq, 1);
        assert!(window.recent.frames.len() == 1);
    }

    #[test]
    fn old_relay_task_does_not_clear_new_relay() {
        let mut window = Window::new();
        window.relay = Some(relay(1, "node-a", 3));
        window.publish_relay(1, 0, true, vec![1]);
        // пришёл новый feed и сменил relay
        let old = window.relay.replace(relay(2, "node-b", 4)).unwrap();
        assert!(window.release_relay(old.id) == false);
        assert_eq!(window.relay.as_ref().map(|r| r.id), Some(2));
        // новый поток кончился — снимаем его
        assert!(window.release_relay(2));
        assert!(window.relay.is_none());
        assert!(window.recent.frames.is_empty());
    }

    #[test]
    fn take_relay_clears_window() {
        let mut window = Window::new();
        window.relay = Some(relay(1, "node-a", 3));
        window.publish_relay(1, 0, true, vec![1]);
        let taken = window.take_relay().unwrap();
        assert_eq!(taken.id, 1);
        assert!(window.recent.frames.is_empty());
        assert!(window.relay.is_none());
    }

    #[test]
    fn route_direct_without_host() {
        let live = vec![("aaa".to_string(), 5)];
        assert_eq!(
            choose_view_route(false, false, 0, &live, None, false),
            ViewRoute::Direct {
                publisher: "aaa".into(),
                gen: 5
            }
        );
    }

    #[test]
    fn route_relay_through_free_host() {
        let live = vec![("aaa".to_string(), 5)];
        assert_eq!(
            choose_view_route(false, false, 0, &live, Some("bbb"), false),
            ViewRoute::Relay {
                host: "bbb".into(),
                publisher: "aaa".into(),
                gen: 5
            }
        );
    }

    #[test]
    fn route_direct_when_host_busy_or_publisher() {
        let live = vec![("aaa".to_string(), 5)];
        // хост сам вещает
        assert!(matches!(
            choose_view_route(false, false, 0, &live, Some("bbb"), true),
            ViewRoute::Direct { .. }
        ));
        // издатель и есть хост
        assert!(matches!(
            choose_view_route(false, false, 0, &live, Some("aaa"), false),
            ViewRoute::Direct { .. }
        ));
    }

    #[test]
    fn host_watches_feed_and_publisher_watches_nothing() {
        let live = vec![("aaa".to_string(), 5)];
        assert_eq!(
            choose_view_route(true, false, 0, &live, None, false),
            ViewRoute::FeedWatch {
                publisher: "aaa".into(),
                gen: 5
            }
        );
        // сам издатель свою же картинку по сети не показывает
        assert_eq!(
            choose_view_route(false, false, 7, &live, Some("bbb"), false),
            ViewRoute::None
        );
        assert_eq!(choose_view_route(false, true, 0, &live, None, false), ViewRoute::None);
        assert_eq!(choose_view_route(false, false, 0, &[], None, false), ViewRoute::None);
    }

    #[test]
    fn route_picks_first_announcement_and_expects_its_source() {
        let live = vec![("aaa".to_string(), 5), ("ccc".to_string(), 9)];
        let route = choose_view_route(false, false, 0, &live, Some("bbb"), false);
        assert_eq!(route.expected_peer(), Some("bbb"));
        match route.target() {
            Some(SubTarget::Relay {
                host,
                gen,
                publisher,
            }) => {
                assert_eq!((host.as_str(), gen, publisher.as_str()), ("bbb", 5, "aaa"));
            }
            other => panic!("ожидался релей, получено {other:?}"),
        }
        let direct = choose_view_route(false, false, 0, &live, None, false);
        assert_eq!(direct.expected_peer(), Some("aaa"));
        assert_eq!(direct.target(), Some(SubTarget::Direct { node: "aaa".into(), gen: 5 }));
    }

    #[test]
    fn feed_target_only_for_selected_publisher() {
        let live: Vec<(String, u32)> = Vec::new();
        assert_eq!(
            choose_feed_target(false, 7, &live, Some("bbb"), false),
            Some(("bbb".to_string(), 7))
        );
        // хост сам вещает
        assert_eq!(choose_feed_target(false, 7, &live, Some("bbb"), true), None);
        // хоста нет
        assert_eq!(choose_feed_target(false, 7, &live, None, false), None);
        // я сам хост
        assert_eq!(choose_feed_target(true, 7, &live, Some("bbb"), false), None);
        // не вещаю
        assert_eq!(choose_feed_target(false, 0, &live, Some("bbb"), false), None);
        // в комнате есть другой живой поток — он и выбран, мой никто не откроет
        let other = vec![("aaa".to_string(), 3)];
        assert_eq!(choose_feed_target(false, 7, &other, Some("bbb"), false), None);
    }

    #[test]
    fn handoff_starts_at_last_keyframe() {
        let mut recent = Recent::new();
        recent.push(false, vec![1]);
        recent.push(false, vec![2]);
        let keyframe = recent.push(true, vec![3]);
        recent.push(false, vec![4]);
        recent.push(false, vec![5]);

        let (tail, last) = recent.handoff();
        assert_eq!(tail.first().unwrap().seq, keyframe.seq);
        assert!(tail.first().unwrap().keyframe);
        assert_eq!(tail.len(), 3);
        assert_eq!(last, 5);
        assert_eq!(tail.iter().map(|f| f.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn handoff_without_keyframe_is_empty() {
        let mut recent = Recent::new();
        recent.push(false, vec![1]);
        recent.push(false, vec![2]);
        let (tail, last) = recent.handoff();
        assert!(tail.is_empty());
        assert_eq!(last, recent.next_seq);
        assert_eq!(last, 2);
    }

    #[test]
    fn old_keyframe_leaves_the_window() {
        let mut recent = Recent::new();
        recent.push(true, vec![0]);
        for _ in 0..RECENT_FRAMES {
            recent.push(false, vec![0]);
        }
        assert!(recent.last_keyframe.is_none());
        assert!(recent.frames.len() <= RECENT_FRAMES);
        let (tail, _) = recent.handoff();
        assert!(tail.is_empty());
    }

    #[test]
    fn clear_drops_everything() {
        let mut recent = Recent::new();
        recent.push(true, vec![1]);
        recent.clear();
        assert!(recent.frames.is_empty());
        assert!(recent.last_keyframe.is_none());
        assert_eq!(recent.bytes, 0);
    }

    #[test]
    fn due_waits_for_buffer() {
        let mut queue = RemoteQueue::new();
        let t0 = Instant::now();
        assert!(queue.push(frame("n", true, 0, t0), Duration::from_millis(2000)));

        let buffer = Duration::from_millis(2000);
        assert!(queue
            .pop_due(buffer, t0 + Duration::from_millis(1999))
            .is_none());
        let shown = queue.pop_due(buffer, t0 + Duration::from_millis(2000)).unwrap();
        assert_eq!(shown.unit.pts_100ns, 0);
        assert!(queue.pop_due(buffer, t0 + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn zero_buffer_shows_immediately() {
        let mut queue = RemoteQueue::new();
        let t0 = Instant::now();
        queue.push(frame("n", true, 1, t0), Duration::ZERO);
        assert!(queue.pop_due(Duration::ZERO, t0).is_some());
    }

    #[test]
    fn due_preserves_order_and_holds_late_frame() {
        let mut queue = RemoteQueue::new();
        let t0 = Instant::now();
        queue.push(frame("n", true, 1, t0), Duration::ZERO);
        // второй кадр опоздал на 500 мс — он придерживает третий
        queue.push(frame("n", false, 2, t0 + Duration::from_millis(500)), Duration::ZERO);
        queue.push(frame("n", false, 3, t0 + Duration::from_millis(510)), Duration::ZERO);

        let now = t0 + Duration::from_millis(400);
        assert_eq!(queue.pop_due(Duration::ZERO, now).unwrap().unit.pts_100ns, 1);
        assert!(queue.pop_due(Duration::ZERO, now).is_none());

        let now = t0 + Duration::from_millis(600);
        assert_eq!(queue.pop_due(Duration::ZERO, now).unwrap().unit.pts_100ns, 2);
        assert_eq!(queue.pop_due(Duration::ZERO, now).unwrap().unit.pts_100ns, 3);
    }

    #[test]
    fn shrinking_buffer_releases_buffered_frames() {
        let mut queue = RemoteQueue::new();
        let t0 = Instant::now();
        queue.push(frame("n", true, 1, t0), Duration::from_secs(5));
        let five = Duration::from_secs(5);
        assert!(queue.pop_due(five, t0).is_none());
        // настройку уменьшили на ходу — кадр уже созрел
        assert!(queue.pop_due(Duration::ZERO, t0).is_some());
    }

    #[test]
    fn overflow_clears_queue_and_waits_for_keyframe() {
        let mut queue = RemoteQueue::new();
        queue.push(frame("n", true, 0, Instant::now()), Duration::ZERO);
        for i in 1..=200 {
            queue.push(frame("n", false, i as i64, Instant::now()), Duration::ZERO);
        }
        // очередь переполнилась и очистилась, P-кадры ждут опору
        assert!(queue.frames.is_empty());
        assert!(queue.resync);
        assert!(!queue.push(frame("n", false, 999, Instant::now()), Duration::ZERO));
        assert!(queue.frames.is_empty());
        assert!(queue.push(frame("n", true, 1000, Instant::now()), Duration::ZERO));
        assert_eq!(queue.frames.len(), 1);
        assert!(!queue.resync);
    }

    #[test]
    fn reset_from_drops_that_stream_only() {
        let mut queue = RemoteQueue::new();
        queue.push(frame("a", true, 1, Instant::now()), Duration::ZERO);
        queue.push(frame("b", true, 2, Instant::now()), Duration::ZERO);
        queue.reset_from("a");
        assert_eq!(queue.frames.len(), 1);
        assert_eq!(queue.frames[0].from, "b");
        assert!(queue.resync);
    }
}
