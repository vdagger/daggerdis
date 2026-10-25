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
        msg: &crate::network::message::NetworkMessage,
    ) -> Result<usize, Box<dyn std::error::Error + Send>> {
        let serialized = serialize_msg(msg);
        let bytes_sent = self.socket.send_to(&serialized, addr).await.map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(bytes_sent)
    }
}