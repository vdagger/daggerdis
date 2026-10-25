use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub enum NetworkMessage {
    Heartbeat { sender_id: u64, is_host: bool },
    JoinRequest { sender_id: u64 },
    PeerListUpdate { peers: Vec<(u64, SocketAddr)> },
    AudioFrame { sender_id: u64, opus_data: Vec<u8> },
}

pub fn serialize_msg(msg: &NetworkMessage) -> Vec<u8> {
    let mut data = Vec::new();
    match msg {
        NetworkMessage::Heartbeat { sender_id, is_host } => {
            data.push(0);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
        }
        NetworkMessage::AudioFrame { sender_id, opus_data } => {
            data.push(3);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(opus_data);
        }
        _ => data.push(99), // Заглушка для остальных типов
    }
    data
}

pub fn deserialize_msg(bytes: &[u8]) -> Option<NetworkMessage> {
    if bytes.is_empty() { return None; }
    match bytes[0] {
        0 if bytes.len() >= 10 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::Heartbeat { sender_id: id, is_host: bytes[9] == 1 })
        }
        3 if bytes.len() >= 9 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::AudioFrame { sender_id: id, opus_data: bytes[9..].to_vec() })
        }
        _ => None,
    }
}