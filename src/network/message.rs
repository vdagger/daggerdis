#[derive(Clone, Debug, PartialEq)]
pub enum NetworkMessage {
    Heartbeat {
        sender_id: u64,
        is_host: bool,
    },
    AudioFrame {
        sender_id: u64,
        seq: u32,
        opus_data: Vec<u8>,
    },
    HostMigration {
        sender_id: u64,
        new_host_id: u64,
    },
    IrohSync {
        sender_id: u64,
        node_id: String,
        is_host: bool,
        /// 0 — пакет от самого узла, 1 — переслан хостом. Дальше одного релея
        /// не идёт: иначе IrohSync ходил бы по кругу между хостами.
        hops: u8,
    },
    FriendRequest {
        sender_id: u64,
        node_id: String,
        nick: String,
    },
    FriendAccept {
        sender_id: u64,
        node_id: String,
        nick: String,
    },
    FriendRemove {
        sender_id: u64,
        node_id: String,
    },
    ChatMessage {
        sender_id: u64,
        text: String,
    },
    /// Звонок: приглашение. Попасть в комнату можно только после `CallAccept`.
    CallInvite {
        sender_id: u64,
        call_id: u32,
        nick: String,
    },
    CallAccept {
        sender_id: u64,
        call_id: u32,
    },
    /// Причина: 0 — отклонил, 1 — занят, 2 — не отвечает/таймаут
    CallDecline {
        sender_id: u64,
        call_id: u32,
        reason: u8,
    },
    CallCancel {
        sender_id: u64,
        call_id: u32,
    },
    CallEnd {
        sender_id: u64,
        call_id: u32,
    },
}

/// Пишет строку как `длина u16 LE` + байты UTF-8 (единый формат для текстовых полей).
fn push_str(data: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    data.extend_from_slice(&(b.len() as u16).to_le_bytes());
    data.extend_from_slice(b);
}

/// Читает строку из `bytes`, продвигая `offset`; `None` при обрезанных данных.
fn read_str(bytes: &[u8], offset: &mut usize) -> Option<String> {
    if *offset + 2 > bytes.len() {
        return None;
    }
    let len = u16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]) as usize;
    *offset += 2;
    if *offset + len > bytes.len() {
        return None;
    }
    let s = String::from_utf8(bytes[*offset..*offset + len].to_vec()).ok()?;
    *offset += len;
    Some(s)
}

pub fn serialize_msg(msg: &NetworkMessage) -> Vec<u8> {
    let mut data = Vec::new();
    match msg {
        NetworkMessage::Heartbeat { sender_id, is_host } => {
            data.push(0);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
        }
        NetworkMessage::AudioFrame {
            sender_id,
            seq,
            opus_data,
        } => {
            data.push(3);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&seq.to_le_bytes());
            data.extend_from_slice(opus_data);
        }
        NetworkMessage::HostMigration {
            sender_id,
            new_host_id,
        } => {
            data.push(4);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&new_host_id.to_le_bytes());
        }
        NetworkMessage::IrohSync {
            sender_id,
            node_id,
            is_host,
            hops,
        } => {
            data.push(5);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
            data.push(*hops);
            let node_bytes = node_id.as_bytes();
            data.extend_from_slice(&(node_bytes.len() as u16).to_le_bytes());
            data.extend_from_slice(node_bytes);
        }
        NetworkMessage::FriendRequest {
            sender_id,
            node_id,
            nick,
        } => {
            data.push(6);
            data.extend_from_slice(&sender_id.to_le_bytes());
            push_str(&mut data, node_id);
            push_str(&mut data, nick);
        }
        NetworkMessage::FriendAccept {
            sender_id,
            node_id,
            nick,
        } => {
            data.push(7);
            data.extend_from_slice(&sender_id.to_le_bytes());
            push_str(&mut data, node_id);
            push_str(&mut data, nick);
        }
        NetworkMessage::FriendRemove { sender_id, node_id } => {
            data.push(8);
            data.extend_from_slice(&sender_id.to_le_bytes());
            push_str(&mut data, node_id);
        }
        NetworkMessage::ChatMessage { sender_id, text } => {
            data.push(9);
            data.extend_from_slice(&sender_id.to_le_bytes());
            push_str(&mut data, text);
        }
        NetworkMessage::CallInvite {
            sender_id,
            call_id,
            nick,
        } => {
            data.push(10);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&call_id.to_le_bytes());
            push_str(&mut data, nick);
        }
        NetworkMessage::CallAccept { sender_id, call_id } => {
            data.push(11);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&call_id.to_le_bytes());
        }
        NetworkMessage::CallDecline {
            sender_id,
            call_id,
            reason,
        } => {
            data.push(12);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&call_id.to_le_bytes());
            data.push(*reason);
        }
        NetworkMessage::CallCancel { sender_id, call_id } => {
            data.push(13);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&call_id.to_le_bytes());
        }
        NetworkMessage::CallEnd { sender_id, call_id } => {
            data.push(14);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&call_id.to_le_bytes());
        }
    }
    data
}

pub fn deserialize_msg(bytes: &[u8]) -> Option<NetworkMessage> {
    if bytes.is_empty() {
        return None;
    }
    match bytes[0] {
        0 if bytes.len() >= 10 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::Heartbeat {
                sender_id: id,
                is_host: bytes[9] == 1,
            })
        }
        3 if bytes.len() >= 13 => {
            let id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let seq = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::AudioFrame {
                sender_id: id,
                seq,
                opus_data: bytes[13..].to_vec(),
            })
        }
        4 if bytes.len() >= 17 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let new_host_id = u64::from_le_bytes(bytes[9..17].try_into().ok()?);
            Some(NetworkMessage::HostMigration {
                sender_id,
                new_host_id,
            })
        }
        5 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let is_host = bytes[9] == 1;
            let hops = bytes[10];
            let len = u16::from_le_bytes([bytes[11], bytes[12]]) as usize;
            if bytes.len() >= 13 + len {
                let node_id = String::from_utf8(bytes[13..13 + len].to_vec()).ok()?;
                Some(NetworkMessage::IrohSync {
                    sender_id,
                    node_id,
                    is_host,
                    hops,
                })
            } else {
                None
            }
        }
        6 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let mut off = 9;
            let node_id = read_str(bytes, &mut off)?;
            let nick = read_str(bytes, &mut off)?;
            Some(NetworkMessage::FriendRequest {
                sender_id,
                node_id,
                nick,
            })
        }
        7 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let mut off = 9;
            let node_id = read_str(bytes, &mut off)?;
            let nick = read_str(bytes, &mut off)?;
            Some(NetworkMessage::FriendAccept {
                sender_id,
                node_id,
                nick,
            })
        }
        8 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let mut off = 9;
            let node_id = read_str(bytes, &mut off)?;
            Some(NetworkMessage::FriendRemove { sender_id, node_id })
        }
        9 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let mut off = 9;
            let text = read_str(bytes, &mut off)?;
            Some(NetworkMessage::ChatMessage { sender_id, text })
        }
        10 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let call_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            let mut off = 13;
            let nick = read_str(bytes, &mut off)?;
            Some(NetworkMessage::CallInvite {
                sender_id,
                call_id,
                nick,
            })
        }
        11 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let call_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallAccept { sender_id, call_id })
        }
        12 if bytes.len() >= 14 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let call_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallDecline {
                sender_id,
                call_id,
                reason: bytes[13],
            })
        }
        13 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let call_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallCancel { sender_id, call_id })
        }
        14 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let call_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallEnd { sender_id, call_id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: NetworkMessage) {
        let bytes = serialize_msg(&msg);
        let back = deserialize_msg(&bytes).expect("пакет должен разбираться");
        assert_eq!(back, msg, "round-trip потерял данные");
    }

    #[test]
    fn roundtrip_all_variants() {
        roundtrip(NetworkMessage::Heartbeat {
            sender_id: 0x0102_0304_0506_0708,
            is_host: true,
        });
        roundtrip(NetworkMessage::AudioFrame {
            sender_id: 7,
            seq: 1234,
            opus_data: vec![1, 2, 3, 255],
        });
        roundtrip(NetworkMessage::HostMigration {
            sender_id: 11,
            new_host_id: 22,
        });
        roundtrip(NetworkMessage::IrohSync {
            sender_id: 33,
            node_id: "ab".repeat(32),
            is_host: true,
            hops: 1,
        });
        roundtrip(NetworkMessage::FriendRequest {
            sender_id: 1,
            node_id: "node".into(),
            nick: "Вася".into(),
        });
        roundtrip(NetworkMessage::FriendAccept {
            sender_id: 2,
            node_id: "node2".into(),
            nick: "Петя".into(),
        });
        roundtrip(NetworkMessage::FriendRemove {
            sender_id: 3,
            node_id: "node3".into(),
        });
        roundtrip(NetworkMessage::ChatMessage {
            sender_id: 4,
            text: "привет, мир".into(),
        });
        roundtrip(NetworkMessage::CallInvite {
            sender_id: 5,
            call_id: 0xDEAD_BEEF,
            nick: "Костя".into(),
        });
        roundtrip(NetworkMessage::CallAccept {
            sender_id: 6,
            call_id: 9,
        });
        roundtrip(NetworkMessage::CallDecline {
            sender_id: 7,
            call_id: 10,
            reason: 2,
        });
        roundtrip(NetworkMessage::CallCancel {
            sender_id: 8,
            call_id: 11,
        });
        roundtrip(NetworkMessage::CallEnd {
            sender_id: 9,
            call_id: 12,
        });
    }

    #[test]
    fn host_migration_needs_17_bytes() {
        let bytes = serialize_msg(&NetworkMessage::HostMigration {
            sender_id: 1,
            new_host_id: 2,
        });
        assert_eq!(bytes.len(), 17);
        assert!(deserialize_msg(&bytes).is_some());
        // старая версия пакета без new_host_id — отбрасывается
        assert!(deserialize_msg(&bytes[..16]).is_none());
    }

    #[test]
    fn iroh_sync_layout() {
        let bytes = serialize_msg(&NetworkMessage::IrohSync {
            sender_id: 0xAA,
            node_id: "node".into(),
            is_host: true,
            hops: 1,
        });
        assert_eq!(&bytes[..1], &[5]);
        assert_eq!(bytes[9], 1); // is_host
        assert_eq!(bytes[10], 1); // hops
        assert_eq!(u16::from_le_bytes([bytes[11], bytes[12]]), 4);
        assert_eq!(&bytes[13..], b"node");

        // обрезанный node_id — отказ, а не пакет с пустым NodeID
        assert!(deserialize_msg(&bytes[..15]).is_none());
        let mut cut = bytes.clone();
        cut.truncate(13);
        assert!(deserialize_msg(&cut).is_none());
    }

    #[test]
    fn truncated_calls_rejected() {
        let accept = serialize_msg(&NetworkMessage::CallAccept {
            sender_id: 1,
            call_id: 2,
        });
        assert!(deserialize_msg(&accept[..12]).is_none());

        let decline = serialize_msg(&NetworkMessage::CallDecline {
            sender_id: 1,
            call_id: 2,
            reason: 1,
        });
        assert!(deserialize_msg(&decline[..13]).is_none());

        let invite = serialize_msg(&NetworkMessage::CallInvite {
            sender_id: 1,
            call_id: 2,
            nick: "ник".into(),
        });
        // никовое поле не влезло целиком
        assert!(deserialize_msg(&invite[..invite.len() - 1]).is_none());
    }

    #[test]
    fn garbage_rejected() {
        assert!(deserialize_msg(&[]).is_none());
        assert!(deserialize_msg(&[99]).is_none());
        assert!(deserialize_msg(&[0]).is_none());
    }
}
