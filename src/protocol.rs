use serde::{Deserialize, Serialize};
use crate::types::Peer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkMessage {
    pub room_id: String,
    pub sender_id: u64,
    pub packet: Packet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Packet {
    Heartbeat,
    AudioData(Vec<u8>),
    JoinRoom,
    RoomState(Vec<Peer>),
    MigrateHost(u64),
}