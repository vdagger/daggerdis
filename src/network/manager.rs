use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use crate::network::message::{serialize_msg, deserialize_msg};
use crate::network::p2p_iroh::IrohP2pManager;

pub struct NetworkManager {
    socket: UdpSocket,
    pub iroh: Option<Arc<IrohP2pManager>>,
}

impl NetworkManager {
    pub async fn new(addr: SocketAddr) -> Result<Self, Box<dyn std::error::Error + Send>> {
        let socket = UdpSocket::bind(addr).await.map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(NetworkManager { socket, iroh: None })
    }

    pub fn from_std(
        socket: std::net::UdpSocket, 
        iroh: Option<Arc<IrohP2pManager>>
    ) -> Result<Self, Box<dyn std::error::Error + Send>> {
        socket.set_nonblocking(true).map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        let socket = UdpSocket::from_std(socket).map_err(|e| -> Box<dyn std::error::Error + Send> { Box::new(e) })?;
        Ok(NetworkManager { socket, iroh })
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