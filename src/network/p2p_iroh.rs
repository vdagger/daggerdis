//! Модуль P2P-транспорта на базе Iroh (QUIC Datagrams + NAT Traversal + Relay)

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use iroh::{Endpoint, EndpointId, endpoint::presets, endpoint::Connection};
use bytes::Bytes;

pub const IROH_ALPN: &[u8] = b"discordrf/1";

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
}

impl IrohP2pManager {
    /// Инициализирует узел Iroh и подключается к сети релеев
    pub async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        println!("[IROH] Инициализация P2P-эндпоинта (пресет N0)...");
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![IROH_ALPN.to_vec()])
            .bind()
            .await?;

        let my_id = endpoint.id();
        let my_node_id = my_id.to_string();
        println!("[IROH] 🚀 Успешный запуск! Ваш NodeID: {}", my_node_id);

        let connections = Arc::new(Mutex::new(HashMap::new()));
        let (incoming_tx, incoming_rx) = mpsc::channel::<IncomingPacket>(500);

        // Фоновый цикл приема входящих подключений
        let ep_accept = endpoint.clone();
        let conns_accept = connections.clone();
        let tx_accept = incoming_tx.clone();
        tokio::spawn(async move {
            println!("[IROH] Фоновый слушатель входящих подключений запущен.");
            while let Some(incoming) = ep_accept.accept().await {
                match incoming.await {
                    Ok(conn) => {
                        let remote_id = conn.remote_id().to_string();
                        println!("[IROH] Принято входящее P2P-подключение от NodeID: {}", remote_id);
                        
                        conns_accept.lock().await.insert(remote_id.clone(), conn.clone());
                        Self::spawn_datagram_receiver(conn, remote_id, tx_accept.clone());
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
        })
    }

    /// Подключается к другому узлу по его строковому NodeID
    pub async fn connect_to_peer(&self, target_id_str: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let trimmed = target_id_str.trim();
        let target_id: EndpointId = trimmed.parse()?;

        println!("[IROH] Подключение к удаленному NodeID: {}...", trimmed);
        let conn = self.endpoint.connect(target_id, IROH_ALPN).await?;
        println!("[IROH] ✅ P2P-соединение установлено с: {}", trimmed);

        self.connections.lock().await.insert(trimmed.to_string(), conn.clone());
        Self::spawn_datagram_receiver(conn, trimmed.to_string(), self.incoming_tx.clone());
        Ok(())
    }

    /// Запускает фоновое чтение датаграмм (голосовых пакетов) из QUIC-соединения
    fn spawn_datagram_receiver(conn: Connection, remote_id: String, tx: mpsc::Sender<IncomingPacket>) {
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

    /// Рассылает байты всем активным P2P-соседям, кроме автора
    pub async fn broadcast_bytes_except(&self, except_node_id: &str, data: &[u8]) {
        let conns = self.connections.lock().await;
        let bytes = Bytes::copy_from_slice(data);
        for (node_id, conn) in conns.iter() {
            if node_id != except_node_id {
                let _ = conn.send_datagram(bytes.clone());
            }
        }
    }

    /// Рассылает байты всем активным P2P-соседям
    pub async fn broadcast_bytes(&self, data: &[u8]) {
        let conns = self.connections.lock().await;
        let bytes = Bytes::copy_from_slice(data);
        for conn in conns.values() {
            let _ = conn.send_datagram(bytes.clone());
        }
    }

    /// Возвращает список NodeID подключенных участников
    pub async fn get_connected_peers(&self) -> Vec<String> {
        self.connections.lock().await.keys().cloned().collect()
    }

    /// Отправляет сырые байты (Opus / NetworkMessage) указанному NodeID
    pub async fn send_bytes(&self, target_id_str: &str, data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let conns = self.connections.lock().await;
        if let Some(conn) = conns.get(target_id_str) {
            let bytes = Bytes::copy_from_slice(data);
            conn.send_datagram(bytes)?;
        }
        Ok(())
    }

    /// Принимает следующий входящий пакет
    pub async fn recv_packet(&self) -> Option<IncomingPacket> {
        self.incoming_rx.lock().await.recv().await
    }
}