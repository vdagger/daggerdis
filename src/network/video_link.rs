//! Видеотранспорт.
//!
//! Отдельное iroh-соединение с ALPN `discordrf-video/1`: у каждого QUIC-соединения
//! свой congestion control, поэтому видео не душит голос. Один uni-стрим на кадр —
//! потеря пакета задерживает только один кадр, без head-of-line блокировки
//! между кадрами.
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

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use super::p2p_iroh::IrohP2pManager;
use crate::video::{EncodedUnit, StreamInfo};

pub const VIDEO_ALPN: &[u8] = b"discordrf-video/1";

/// Верхняя граница одного кадра: 1440p-кейфрейм с запасом.
const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Пришедший по сети кадр.
pub struct RemoteFrame {
    pub from: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sequence_header: Vec<u8>,
    pub unit: EncodedUnit,
}

pub struct VideoLink {
    p2p: Arc<IrohP2pManager>,
    outgoing: broadcast::Sender<Arc<Vec<u8>>>,
    events_tx: mpsc::UnboundedSender<RemoteFrame>,
    events_rx: std::sync::Mutex<mpsc::UnboundedReceiver<RemoteFrame>>,
}

impl VideoLink {
    pub fn spawn(p2p: Arc<IrohP2pManager>, rt: tokio::runtime::Handle) -> Arc<Self> {
        let (outgoing, _) = broadcast::channel(8);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let link = Arc::new(Self {
            p2p: p2p.clone(),
            outgoing,
            events_tx,
            events_rx: std::sync::Mutex::new(events_rx),
        });
        let l = link.clone();
        rt.spawn(async move { l.accept_loop().await });
        link
    }

    /// Принимает входящие видео-соединения и читает кадры из uni-стримов.
    async fn accept_loop(&self) {
        while let Some(conn) = self.p2p.recv_video_conn().await {
            let from = conn.remote_id().to_string();
            println!("[ВИДЕО] Входящее видео-соединение от {}", from);
            let tx = self.events_tx.clone();
            tokio::spawn(async move {
                loop {
                    let Ok(mut recv) = conn.accept_uni().await else {
                        break;
                    };
                    let Ok(data) = recv.read_to_end(MAX_FRAME).await else {
                        continue;
                    };
                    match parse_frame(&data, &from) {
                        Some(frame) => {
                            if tx.send(frame).is_err() {
                                break;
                            }
                        }
                        None => eprintln!("[ВИДЕО] Битый кадр от {}", from),
                    }
                }
                println!("[ВИДЕО] Поток от {} завершён", from);
            });
        }
    }

    /// Открывает видео-соединение к зрителю и шлёт ему все публикуемые кадры.
    pub async fn add_viewer(self: &Arc<Self>, node_id: String) {
        let trimmed = node_id.trim().to_string();
        let Ok(target) = trimmed.parse::<iroh::EndpointId>() else {
            eprintln!("[ВИДЕО] Некорректный NodeID зрителя: {trimmed}");
            return;
        };
        match self.p2p.endpoint().connect(target, VIDEO_ALPN).await {
            Ok(conn) => {
                println!("[ВИДЕО] Зритель подключён: {}", trimmed);
                let mut rx = self.outgoing.subscribe();
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(frame) => match conn.open_uni().await {
                                Ok(mut send) => {
                                    let _ = send.write_all(&frame).await;
                                    let _ = send.finish();
                                }
                                Err(_) => break,
                            },
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => break,
                        }
                    }
                });
            }
            Err(e) => eprintln!("[ВИДЕО] Не удалось подключиться к {trimmed}: {e}"),
        }
    }

    /// Публикует кадр всем подключённым зрителям.
    pub fn publish(&self, unit: &EncodedUnit, info: &StreamInfo) {
        if self.outgoing.receiver_count() == 0 {
            return;
        }
        let _ = self.outgoing.send(Arc::new(encode_frame(unit, info)));
    }

    /// Забирает пришедшие кадры (для GUI, без блокировки).
    pub fn try_recv_remote(&self) -> Option<RemoteFrame> {
        self.events_rx.lock().ok()?.try_recv().ok()
    }
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
    v.push(1);
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
    if data.len() < 28 || data[0] != 1 {
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
    })
}
