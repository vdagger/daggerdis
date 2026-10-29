use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub enum NetworkMessage {
    Heartbeat { sender_id: u64, is_host: bool },
    JoinRequest { sender_id: u64 },
    PeerListUpdate { peers: Vec<(u64, SocketAddr)> },
    AudioFrame { sender_id: u64, opus_data: Vec<u8> },
    HostMigration { new_host_id: u64 },
    IrohSync { sender_id: u64, node_id: String, is_host: bool },
}

pub fn serialize_msg(msg: &NetworkMessage) -> Vec<u8> {
    let mut data = Vec::new();
    match msg {
        NetworkMessage::Heartbeat { sender_id, is_host } => {
            data.push(0);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
        }
        NetworkMessage::JoinRequest { sender_id } => {
            data.push(1);
            data.extend_from_slice(&sender_id.to_le_bytes());
        }
        NetworkMessage::PeerListUpdate { peers } => {
            data.push(2);
            data.extend_from_slice(&(peers.len() as u16).to_le_bytes());
            for (id, socket_addr) in peers {
                data.extend_from_slice(&id.to_le_bytes());
                
                // Serialize SocketAddr
                match socket_addr {
                    SocketAddr::V4(addr) => {
                        data.push(0); // IPv4 flag
                        data.extend_from_slice(&addr.ip().octets()); // 4 bytes for IPv4
                        data.extend_from_slice(&(addr.port()).to_be_bytes()); // 2 bytes for port
                    }
                    SocketAddr::V6(addr) => {
                        data.push(1); // IPv6 flag
                        data.extend_from_slice(&addr.ip().octets()); // 16 bytes for IPv6
                        data.extend_from_slice(&(addr.port()).to_be_bytes()); // 2 bytes for port
                    }
                }
            }
        }
        NetworkMessage::AudioFrame { sender_id, opus_data } => {
            data.push(3);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(opus_data);
        }
        NetworkMessage::HostMigration { new_host_id } => {
            data.push(4);
            data.extend_from_slice(&new_host_id.to_le_bytes());
        }
        NetworkMessage::IrohSync { sender_id, node_id, is_host } => {
            data.push(5);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
            let node_bytes = node_id.as_bytes();
            data.extend_from_slice(&(node_bytes.len() as u16).to_le_bytes());
            data.extend_from_slice(node_bytes);
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
        1 if bytes.len() >= 9 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::JoinRequest { sender_id: id })
        }
        2 if bytes.len() >= 3 => {
            let count = u16::from_le_bytes([bytes[1], bytes[2]]);
            let mut peers = Vec::new();
            let mut offset = 3;
            
            for _ in 0..count {
                if offset + 8 >= bytes.len() {
                    return None; // Недостаточно данных для ID
                }
                
                let id = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
                offset += 8;
                
                if offset + 1 >= bytes.len() {
                    return None; // Недостаточно данных для типа IP
                }
                
                let ip_type = bytes[offset];
                offset += 1;
                
                match ip_type {
                    0 if offset + 6 <= bytes.len() => { // IPv4
                        let ip_bytes: [u8; 4] = bytes[offset..offset+4].try_into().ok()?;
                        let port_bytes: [u8; 2] = bytes[offset+4..offset+6].try_into().ok()?;
                        let port = u16::from_be_bytes(port_bytes);
                        let socket_addr = std::net::SocketAddrV4::new(
                            std::net::Ipv4Addr::from(ip_bytes),
                            port
                        );
                        peers.push((id, socket_addr.into()));
                        offset += 6;
                    }
                    1 if offset + 18 <= bytes.len() => { // IPv6
                        let ip_bytes: [u8; 16] = bytes[offset..offset+16].try_into().ok()?;
                        let port_bytes: [u8; 2] = bytes[offset+16..offset+18].try_into().ok()?;
                        let port = u16::from_be_bytes(port_bytes);
                        let socket_addr = std::net::SocketAddrV6::new(
                            std::net::Ipv6Addr::from(ip_bytes),
                            port,
                            0, // flowinfo
                            0  // scope_id
                        );
                        peers.push((id, socket_addr.into()));
                        offset += 18;
                    }
                    _ => return None, // Неподдерживаемый тип IP
                }
            }
            
            if offset == bytes.len() {
                Some(NetworkMessage::PeerListUpdate { peers })
            } else {
                None // Неправильный формат данных
            }
        }
        3 if bytes.len() >= 9 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::AudioFrame { sender_id: id, opus_data: bytes[9..].to_vec() })
        }
        4 if bytes.len() >= 9 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::HostMigration { new_host_id: id })
        }
        5 if bytes.len() >= 12 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let is_host = bytes[9] == 1;
            let len = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
            if bytes.len() >= 12 + len {
                let node_id = String::from_utf8(bytes[12..12+len].to_vec()).ok()?;
                Some(NetworkMessage::IrohSync { sender_id, node_id, is_host })
            } else {
                None
            }
        }
        _ => None,
    }
}