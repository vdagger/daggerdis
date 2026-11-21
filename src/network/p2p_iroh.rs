//! Модуль P2P-транспорта на базе Iroh (QUIC Datagrams + NAT Traversal + Relay)

use bytes::Bytes;
use iroh::{endpoint::presets, endpoint::Connection, Endpoint, EndpointId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

pub const IROH_ALPN: &[u8] = b"discordrf/1";

use crate::network::video_link::VIDEO_ALPN;

/// Пакет, полученный от удаленного участника
pub struct IncomingPacket {
    pub sender_id: String,
    pub data: Vec<u8>,
}

/// Управляет P2P-соединениями через протокол Iroh
pub struct IrohP2pManager {
    endpoint: Endpoint,
    pub my_node_id: String,
    connections: Arc<Mutex<HashMap<String, Connection>>>,
    incoming_tx: mpsc::Sender<IncomingPacket>,
    incoming_rx: Arc<Mutex<mpsc::Receiver<IncomingPacket>>>,
    /// Входящие соединения видео-ALPN: их обрабатывает видеотранспорт.
    video_rx: Arc<Mutex<mpsc::Receiver<Connection>>>,
}

impl IrohP2pManager {
    /// Инициализирует узел Iroh и подключается к сети релеев.
    /// `secret_key` закрепляет личность: NodeID не меняется между запусками.
    pub async fn new(
        secret_key: iroh::SecretKey,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        println!("[IROH] Инициализация P2P-эндпоинта (пресет N0, закреплённый ключ)...");
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            // голос живёт на discordrf/1, видео — на своём ALPN, чтобы
            // congestion control видео не душил голос
            .alpns(vec![IROH_ALPN.to_vec(), VIDEO_ALPN.to_vec()])
            .bind()
            .await?;

        let my_id = endpoint.id();
        let my_node_id = my_id.to_string();
        println!("[IROH] 🚀 Успешный запуск! Ваш NodeID: {}", my_node_id);

        let connections = Arc::new(Mutex::new(HashMap::new()));
        let (incoming_tx, incoming_rx) = mpsc::channel::<IncomingPacket>(500);
        let (video_tx, video_rx) = mpsc::channel::<Connection>(8);

        // Фоновый цикл приема входящих подключений
        let ep_accept = endpoint.clone();
        let conns_accept = connections.clone();
        let tx_accept = incoming_tx.clone();
        let video_tx_accept = video_tx.clone();
        tokio::spawn(async move {
            println!("[IROH] Фоновый слушатель входящих подключений запущен.");
            while let Some(incoming) = ep_accept.accept().await {
                match incoming.await {
                    Ok(conn) => {
                        // видео-соединения обслуживает отдельный транспорт
                        if conn.alpn() == VIDEO_ALPN {
                            let _ = video_tx_accept.send(conn).await;
                            continue;
                        }
                        let remote_id = conn.remote_id().to_string();
                        println!(
                            "[IROH] Принято входящее P2P-подключение от NodeID: {}",
                            remote_id
                        );

                        conns_accept
                            .lock()
                            .await
                            .insert(remote_id.clone(), conn.clone());
                        Self::spawn_datagram_receiver(conn.clone(), remote_id.clone(), tx_accept.clone());
                        // Надёжный путь для сообщений (чат, звонки): датаграм
                        // может потеряться, поток — нет.
                        Self::spawn_stream_receiver(conn, remote_id, tx_accept.clone());
                    }
                    Err(e) => {
                        eprintln!("[IROH] Ошибка входящего рукопожатия: {}", e);
                    }
                }
            }
        });

        Ok(Self {
            endpoint,
            my_node_id,
            connections,
            incoming_tx,
            incoming_rx: Arc::new(Mutex::new(incoming_rx)),
            video_rx: Arc::new(Mutex::new(video_rx)),
        })
    }

    /// Подключается к другому узлу по его строковому NodeID
    pub async fn connect_to_peer(
        &self,
        target_id_str: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let trimmed = target_id_str.trim();
        let target_id: EndpointId = trimmed.parse()?;

        println!("[IROH] Подключение к удаленному NodeID: {}...", trimmed);
        let conn = self.endpoint.connect(target_id, IROH_ALPN).await?;
        println!("[IROH] ✅ P2P-соединение установлено с: {}", trimmed);

        self.connections
            .lock()
            .await
            .insert(trimmed.to_string(), conn.clone());
        Self::spawn_datagram_receiver(conn.clone(), trimmed.to_string(), self.incoming_tx.clone());
        Self::spawn_stream_receiver(conn, trimmed.to_string(), self.incoming_tx.clone());
        Ok(())
    }

    /// Запускает фоновое чтение датаграмм (голосовых пакетов) из QUIC-соединения
    fn spawn_datagram_receiver(
        conn: Connection,
        remote_id: String,
        tx: mpsc::Sender<IncomingPacket>,
    ) {
        tokio::spawn(async move {
            loop {
                match conn.read_datagram().await {
                    Ok(dgram) => {
                        let pkt = IncomingPacket {
                            sender_id: remote_id.clone(),
                            data: dgram.to_vec(),
                        };
                        if tx.send(pkt).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        println!("[IROH] P2P-соединение с {} разорвано.", remote_id);
                        break;
                    }
                }
            }
        });
    }

    /// Читает надёжные uni-стримы: сюда приходит то, что не поместилось в
    /// датаграм или потерялось бы в нём (чат, приглашения звонка).
    fn spawn_stream_receiver(
        conn: Connection,
        remote_id: String,
        tx: mpsc::Sender<IncomingPacket>,
    ) {
        tokio::spawn(async move {
            loop {
                let Ok(mut recv) = conn.accept_uni().await else {
                    break;
                };
                // Сообщения маленькие; больше 64 КиБ — точно мусор
                let Ok(data) = recv.read_to_end(64 * 1024).await else {
                    continue;
                };
                if data.is_empty() {
                    continue;
                }
                let pkt = IncomingPacket {
                    sender_id: remote_id.clone(),
                    data,
                };
                if tx.send(pkt).await.is_err() {
                    break;
                }
            }
        });
    }

    /// Закрывает P2P-соединение с узлом. Нужно по окончании звонка: пока
    /// соединение живо, собеседник раз в секунду присылает IrohSync и снова
    /// попадает в комнату, даже если мы его удалили.
    pub async fn disconnect_peer(&self, node_id: &str) {
        let conn = self.connections.lock().await.remove(node_id);
        if let Some(conn) = conn {
            println!("[IROH] Соединение с {} закрыто.", node_id);
            conn.close(0u32.into(), b"call ended");
        }
    }

    /// Выкидывает соединения, которые уже закрыты (пир перезапустился или
    /// соединение уснуло по idle timeout). Иначе они висят в карте вечно:
    /// `get_connected_peers` считает их живыми, звонки уходят «в никуда»,
    /// а авто-связывание думает, что пир уже на связи.
    async fn prune_dead(&self) {
        let mut conns = self.connections.lock().await;
        conns.retain(|_, conn| conn.close_reason().is_none());
    }

    /// Рассылает байты всем активным P2P-соседям, кроме автора
    pub async fn broadcast_bytes_except(&self, except_node_id: &str, data: &[u8]) {
        let mut conns = self.connections.lock().await;
        conns.retain(|_, conn| conn.close_reason().is_none());
        let bytes = Bytes::copy_from_slice(data);
        for (node_id, conn) in conns.iter() {
            if node_id != except_node_id {
                let _ = conn.send_datagram(bytes.clone());
            }
        }
    }

    /// Рассылает байты всем активным P2P-соседям
    pub async fn broadcast_bytes(&self, data: &[u8]) {
        let mut conns = self.connections.lock().await;
        conns.retain(|_, conn| conn.close_reason().is_none());
        let bytes = Bytes::copy_from_slice(data);
        for conn in conns.values() {
            let _ = conn.send_datagram(bytes.clone());
        }
    }

    /// Возвращает список NodeID подключенных участников
    pub async fn get_connected_peers(&self) -> Vec<String> {
        self.prune_dead().await;
        self.connections.lock().await.keys().cloned().collect()
    }

    /// Состояние путей до подключённых узлов: RTT, потери, окно, «напрямую или
    /// через релей».
    ///
    /// RTT считает сам QUIC по своему пути, а не приложение пингами: это то же
    /// число, по которому работают таймеры повторов, то есть честная задержка
    /// транспорта. Метод синхронный (`try_lock`): его зовёт GUI каждый кадр, и
    /// пропустить кадр дешевле, чем ждать карту соединений.
    pub fn link_stats(&self) -> Vec<crate::diagnostics::LinkLine> {
        let Ok(conns) = self.connections.try_lock() else {
            return Vec::new();
        };
        conns
            .iter()
            .filter(|(_, conn)| conn.close_reason().is_none())
            .filter_map(|(node_id, conn)| {
                let paths = conn.paths();
                // Показываем выбранный путь: именно по нему идут данные.
                // Запасной релей в списке есть, но «через релей» ещё не значит
                // «идём через релей», и путать это в диагностике нельзя.
                let path = paths
                    .iter()
                    .find(|p| p.is_selected())
                    .or_else(|| paths.iter().next())?;
                let stats = path.stats();
                Some(crate::diagnostics::LinkLine {
                    node_id: node_id.clone(),
                    relay: path.is_relay(),
                    rtt_ms: stats.rtt.as_millis() as u64,
                    lost_packets: stats.lost_packets,
                    cwnd: stats.cwnd,
                    bandwidth_bps: stats.bandwidth_estimate,
                })
            })
            .collect()
    }

    /// Отправляет сырые байты (Opus / NetworkMessage) указанному NodeID.
    ///
    /// Датаграм уходит только в живое соединение. Раньше запись в закрытое
    /// (пир перезапустился или соединение уснуло по idle timeout) молча
    /// возвращала Ok, и «Позвонить» звонил в пустоту: у звонящего таймер шёл,
    /// у собеседника — тишина. Теперь мёртвое соединение выбрасывается,
    /// переподключение и повтор, а если датаграм так и не прошёл — надёжный
    /// uni-стрим (для чата и звонков потеря недопустима).
    pub async fn send_bytes(
        &self,
        target_id_str: &str,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let bytes = Bytes::copy_from_slice(data);
        let target = target_id_str.trim().to_string();

        let mut stale = false;
        {
            let conns = self.connections.lock().await;
            match conns.get(&target) {
                Some(conn) if conn.close_reason().is_none() => {
                    if conn.send_datagram(bytes.clone()).is_ok() {
                        return Ok(());
                    }
                    stale = true;
                }
                Some(_) => stale = true,
                None => {}
            }
        }
        if stale {
            self.connections.lock().await.remove(&target);
        }

        let conn = {
            let existing = self.connections.lock().await.get(&target).cloned();
            match existing {
                Some(conn) if conn.close_reason().is_none() => conn,
                _ => {
                    self.connect_to_peer(&target).await?;
                    self.connections
                        .lock()
                        .await
                        .get(&target)
                        .cloned()
                        .ok_or("соединение не установлено")?
                }
            }
        };
        if conn.send_datagram(bytes.clone()).is_ok() {
            return Ok(());
        }
        let mut send = conn.open_uni().await?;
        send.write_all(&bytes).await?;
        let _ = send.finish();
        Ok(())
    }

    /// Закрывает все соединения — «выйти из эфира».
    pub async fn disconnect_all(&self) {
        let conns: Vec<(String, Connection)> = self.connections.lock().await.drain().collect();
        for (node_id, conn) in conns {
            println!("[IROH] Соединение с {} закрыто (выход из эфира).", node_id);
            conn.close(0u32.into(), b"left room");
        }
    }

    /// Принимает следующий входящий пакет
    pub async fn recv_packet(&self) -> Option<IncomingPacket> {
        self.incoming_rx.lock().await.recv().await
    }

    /// Входящее видео-соединение (ALPN discordrf-video/1).
    pub async fn recv_video_conn(&self) -> Option<Connection> {
        self.video_rx.lock().await.recv().await
    }

    /// Эндпоинт нужен видеотранспорту для исходящих соединений.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}
