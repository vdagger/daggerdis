use std::net::SocketAddr;
use tokio::net::UdpSocket;
use crate::network::message::{serialize_msg, deserialize_msg};

pub struct NetworkManager {
    socket: UdpSocket,
}

impl NetworkManager {
    pub async fn new(addr: SocketAddr) -> Result<Self, Box<dyn std::error::Error + Send>> {
        let socket = UdpSocket::bind(addr).await.map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(NetworkManager { socket })
    }

    pub async fn send_audio_packet(
        &self,
        addr: SocketAddr,
        data: &[u8],
    ) -> Result<usize, Box<dyn std::error::Error + Send>> {
        let bytes_sent = self.socket.send_to(data, addr).await.map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(bytes_sent)
    }

    pub async fn recv_audio_packet(
        &self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer).await
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub async fn send_packet(
        &self,
        addr: SocketAddr,
        msg: &crate::protocol::NetworkMessage,
    ) -> Result<usize, Box<dyn std::error::Error + Send>> {
        let mesh_msg = match &msg.packet {
            crate::protocol::Packet::JoinRoom => {
                crate::network::message::NetworkMessage::JoinRequest { 
                    sender_id: msg.sender_id 
                }
            }
            crate::protocol::Packet::AudioData(bytes) => {
                crate::network::message::NetworkMessage::AudioFrame { 
                    sender_id: msg.sender_id, 
                    opus_data: bytes.clone() 
                }
            }
            crate::protocol::Packet::Heartbeat => {
                crate::network::message::NetworkMessage::Heartbeat {
                    sender_id: msg.sender_id,
                    is_host: true,
                }
            }
            crate::protocol::Packet::MigrateHost(new_host_id) => {
                crate::network::message::NetworkMessage::HostMigration {
                    new_host_id: *new_host_id,
                }
            }
            _ => {
                // Для других типов пакетов также можно создать соответствующие сообщения
                // Но для текущей задачи реализуем только те, которые указаны
                crate::network::message::NetworkMessage::AudioFrame { 
                    sender_id: msg.sender_id, 
                    opus_data: vec![] 
                }
            }
        };
        
        let serialized = serialize_msg(&mesh_msg);
        let bytes_sent = self.socket.send_to(&serialized, addr).await.map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(bytes_sent)
    }
}