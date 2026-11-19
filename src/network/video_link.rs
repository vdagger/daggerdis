//! Видеотранспорт.
//!
//! Отдельное iroh-соединение с ALPN `discordrf-video/1`: у каждого QUIC-соединения
//! свой congestion control, поэтому видео не душит голос. Один uni-стрим на кадр —
//! потеря пакета задерживает только один кадр, без head-of-line блокировки
//! между кадрами.
//!
//! Соединение открывает зритель — так вошедший в комнату после старта тоже
//! получает поток. Первый bi-стрим — подписка:
//! ```text
//! зритель →  u8 kind=2, u32 gen
//! издатель → u8 kind=3, u8 status (0 — принято, 1 — нет такой трансляции), u32 gen
//! ```
//! Дальше издатель шлёт кадры uni-стримами. Выход зрителя виден по
//! `Connection::closed()`: свой control-стрим зритель закрывает сразу после подписки.
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
use crate::video::{EncodedUnit, StreamInfo};

pub const VIDEO_ALPN: &[u8] = b"discordrf-video/1";

/// Верхняя граница одного кадра: 1440p-кейфрейм с запасом.
const MAX_FRAME: usize = 8 * 1024 * 1024;

const KIND_FRAME: u8 = 1;
const KIND_SUBSCRIBE: u8 = 2;
const KIND_ACK: u8 = 3;

const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Не долбим издателя, который только что отказал или не ответил.
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

/// Кадр, опубликованный издателем. Один объект на кадр делят broadcast и
/// окно недавних кадров, поэтому «подписка + хвост без дыр» — атомарно.
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

/// Открытое видео-соединение к одному зрителю. Соединение живёт внутри
/// задачи отправки; `stop` её будит, и вместе с задачей освобождается
/// QUIC-соединение и его буферы.
struct Viewer {
    id: u64,
    stop: watch::Sender<bool>,
    alive: Arc<AtomicBool>,
}

/// Текущая подписка зрителя: ровно одна, при смене источника старую закрываем.
struct ActiveSub {
    id: u64,
    node: String,
    gen: u32,
    stop: watch::Sender<bool>,
}

#[derive(Default)]
struct ViewerState {
    active: Option<ActiveSub>,
    /// Последняя неудача: (NodeID, поколение, когда). Повтор не раньше
    /// `RETRY_AFTER` и только для того же потока.
    last_fail: Option<(String, u32, Instant)>,
}

pub struct VideoLink {
    p2p: Arc<IrohP2pManager>,
    rt: tokio::runtime::Handle,
    /// Кто сейчас в эфире: сюда издатель кладёт номер своей трансляции,
    /// сюда же пишутся анонсы соседей из IrohSync.
    directory: Arc<VideoDirectory>,
    /// Номер предыдущей трансляции: перезапуск в ту же секунду обязан дать
    /// другой gen, иначе зритель не заметит подмену.
    last_gen: AtomicU32,
    outgoing: broadcast::Sender<Arc<Published>>,
    recent: std::sync::Mutex<Recent>,
    remote: std::sync::Mutex<RemoteQueue>,
    buffer_ms: AtomicU32,
    viewing: std::sync::Mutex<ViewerState>,
    next_id: AtomicU64,
    viewers: std::sync::Mutex<Vec<Viewer>>,
}

impl VideoLink {
    pub fn spawn(
        p2p: Arc<IrohP2pManager>,
        rt: tokio::runtime::Handle,
        directory: Arc<VideoDirectory>,
    ) -> Arc<Self> {
        let (outgoing, _) = broadcast::channel(BROADCAST_FRAMES);
        let link = Arc::new(Self {
            p2p,
            rt,
            directory,
            last_gen: AtomicU32::new(0),
            outgoing,
            recent: std::sync::Mutex::new(Recent::new()),
            remote: std::sync::Mutex::new(RemoteQueue::new()),
            buffer_ms: AtomicU32::new(DEFAULT_BUFFER_MS),
            viewing: std::sync::Mutex::new(ViewerState::default()),
            next_id: AtomicU64::new(0),
            viewers: std::sync::Mutex::new(Vec::new()),
        });
        let l = link.clone();
        link.rt.spawn(async move { l.accept_loop().await });
        link
    }

    /// Принимает входящие соединения зрителей и разбирает подписку.
    async fn accept_loop(self: Arc<Self>) {
        while let Some(conn) = self.p2p.recv_video_conn().await {
            let me = self.clone();
            self.rt
                .spawn(async move { me.handle_subscribe(conn).await });
        }
    }

    async fn handle_subscribe(self: Arc<Self>, conn: Connection) {
        let from = conn.remote_id().to_string();
        let pair = timeout(SUBSCRIBE_TIMEOUT, async { conn.accept_bi().await }).await;
        let Ok(Ok((mut send, mut recv))) = pair else {
            // Старый зритель шлёт кадры без подписки — ему тут не место.
            conn.close(0u32.into(), b"no-subscribe");
            return;
        };
        let request = timeout(SUBSCRIBE_TIMEOUT, recv.read_to_end(64)).await;
        let requested_gen = match request {
            Ok(Ok(data)) => parse_subscribe(&data),
            _ => None,
        };
        let my_gen = self.directory.my_gen();
        let accepted = matches!(requested_gen, Some(gen) if gen == my_gen && my_gen != 0);

        let ack = encode_ack(if accepted { 0 } else { 1 }, my_gen);
        let _ = send.write_all(&ack).await;
        let _ = send.finish();

        if !accepted {
            println!(
                "[ВИДЕО] Подписка от {} отклонена (запрошено {:?}, в эфире {})",
                from, requested_gen, my_gen
            );
            conn.close(0u32.into(), b"no-stream");
            return;
        }
        println!("[ВИДЕО] Зритель подписался: {}", from);
        self.start_sender(conn, from);
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
            me.send_frames(conn, stop_rx).await;
            alive.store(false, Ordering::Relaxed);
            if let Ok(mut viewers) = me.viewers.lock() {
                viewers.retain(|v| v.id != id);
            }
            println!("[ВИДЕО] Зритель {} отключился", node);
        });
    }

    /// Подписывается на broadcast и снимает окно недавних кадров под одним
    /// мьютексом с `publish`: хвост и последующие кадры стыкуются без дыр.
    fn handoff(&self) -> (broadcast::Receiver<Arc<Published>>, Vec<Arc<Published>>, u32) {
        match self.recent.lock() {
            Ok(recent) => {
                let rx = self.outgoing.subscribe();
                let (tail, last) = recent.handoff();
                (rx, tail, last)
            }
            Err(_) => (self.outgoing.subscribe(), Vec::new(), 0),
        }
    }

    async fn send_frames(self: &Arc<Self>, conn: Connection, mut stop: watch::Receiver<bool>) {
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
    /// по нему зрители отличают перезапуск.
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
        if let Ok(mut recent) = self.recent.lock() {
            recent.clear();
        }
        gen
    }

    /// Останавливает трансляцию: анонс gen 0, зрители отключаются.
    pub fn end_stream(&self) {
        self.directory.set_my_gen(0);
        self.stop_viewers();
        if let Ok(mut recent) = self.recent.lock() {
            recent.clear();
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

    /// Публикует кадр: нумерует, кладёт в окно и рассылает подписчикам —
    /// всё под одним замком, чтобы подписка получала цепочку без дыр.
    /// Окно ведётся всегда: иначе вошедшему зрителю пришлось бы ждать
    /// следующего IDR.
    pub fn publish(&self, unit: &EncodedUnit, info: &StreamInfo) {
        if self.directory.my_gen() == 0 {
            return;
        }
        let data = encode_frame(unit, info);
        let Ok(mut recent) = self.recent.lock() else {
            return;
        };
        let frame = recent.push(unit.keyframe, data);
        let _ = self.outgoing.send(frame);
    }

    pub fn set_buffer_ms(&self, ms: u32) {
        self.buffer_ms.store(ms.min(5000), Ordering::Relaxed);
    }

    pub fn buffer_ms(&self) -> u32 {
        self.buffer_ms.load(Ordering::Relaxed)
    }

    /// Подстраивает подписку под живые трансляции комнаты. `target` — поток,
    /// который сейчас нужно смотреть (или `None`). Вызывается каждый кадр:
    /// активная подписка на тот же поток не трогается.
    pub fn sync_viewing(self: &Arc<Self>, target: Option<(String, u32)>) {
        let Ok(mut state) = self.viewing.lock() else {
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
        state.active = Some(ActiveSub {
            id,
            node: node.clone(),
            gen,
            stop,
        });
        drop(state);

        let me = self.clone();
        self.rt.spawn(async move {
            me.run_subscription(id, node, gen, stop_rx).await;
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

    fn clear_remote(&self) {
        if let Ok(mut q) = self.remote.lock() {
            q.frames.clear();
            q.bytes = 0;
            q.resync = true;
        }
    }

    async fn run_subscription(
        self: Arc<Self>,
        id: u64,
        node: String,
        gen: u32,
        mut stop: watch::Receiver<bool>,
    ) {
        let received = self.connect_and_read(&node, gen, &mut stop).await;
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
        if send.write_all(&encode_subscribe(gen)).await.is_err() {
            return false;
        }
        let _ = send.finish();

        let ack = timeout(SUBSCRIBE_TIMEOUT, recv.read_to_end(64)).await;
        let accepted = match ack {
            Ok(Ok(data)) => parse_ack(&data).map(|(status, _)| status == 0),
            _ => None,
        };
        if accepted != Some(true) {
            println!(
                "[ВИДЕО] Издатель {} не принял подписку (поколение {gen})",
                node
            );
            conn.close(0u32.into(), b"rejected");
            return false;
        }
        println!("[ВИДЕО] Подписка принята: {} (поколение {gen})", node);
        // Кадры прошлой подписки к новому потоку не относятся.
        if let Ok(mut q) = self.remote.lock() {
            q.reset_from(node);
        }

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
        if let Ok(mut q) = self.remote.lock() {
            q.reset_from(node);
        }
        conn.close(0u32.into(), b"stream ended");
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

fn encode_subscribe(gen: u32) -> [u8; 5] {
    let mut out = [0u8; 5];
    out[0] = KIND_SUBSCRIBE;
    out[1..].copy_from_slice(&gen.to_le_bytes());
    out
}

fn parse_subscribe(data: &[u8]) -> Option<u32> {
    if data.len() < 5 || data[0] != KIND_SUBSCRIBE {
        return None;
    }
    Some(u32::from_le_bytes(data[1..5].try_into().ok()?))
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
    let rest = &data[28..];
    if rest.len() < seq_len + len {
        return None;
    }
    Some(RemoteFrame {
        from: from.to_string(),
        width,
        height,
        fps,
        sequence_header: rest[..seq_len].to_vec(),
        unit: EncodedUnit {
            data: rest[seq_len..seq_len + len].to_vec(),
            keyframe: flags & 1 != 0,
            pts_100ns: pts,
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

    #[test]
    fn subscribe_roundtrip_and_garbage() {
        let bytes = encode_subscribe(0xDEAD_BEEF);
        assert_eq!(bytes[0], KIND_SUBSCRIBE);
        assert_eq!(parse_subscribe(&bytes), Some(0xDEAD_BEEF));
        assert_eq!(parse_subscribe(&bytes[..4]), None);
        // ответ издателя подпиской не считается
        assert_eq!(parse_subscribe(&encode_ack(0, 7)), None);
        assert_eq!(parse_subscribe(&[]), None);
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
