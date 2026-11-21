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
        /// Номер текущей трансляции экрана: 0 — не транслирует. По нему
        /// зритель находит живой поток и переподписывается при перезапуске.
        video_gen: u32,
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
        room_id: u32,
        nick: String,
    },
    CallAccept {
        sender_id: u64,
        room_id: u32,
    },
    /// Причина: 0 — отклонил, 1 — занят, 2 — не отвечает/таймаут
    CallDecline {
        sender_id: u64,
        room_id: u32,
        reason: u8,
    },
    CallCancel {
        sender_id: u64,
        room_id: u32,
    },
    CallEnd {
        sender_id: u64,
        room_id: u32,
    },
    /// «Я в сети»: друг проверяет, доступен ли узел. Ответ — `PresencePong`.
    /// Отдельный пакет, а не Heartbeat: тот живёт только внутри комнаты.
    PresencePing {
        sender_id: u64,
    },
    /// Ответ на `PresencePing` — сам пинг отправителю тоже не нужен.
    PresencePong {
        sender_id: u64,
    },
    /// Кто ещё в комнате: пригласивший отдаёт список участников тому, кто
    /// только что принял приглашение.
    ///
    /// Без этого список комнаты не собирается: новый участник соединён только
    /// с пригласившим, а остальные узнают о нём лишь когда он сам им напишет.
    /// Полный mesh строится по этому списку (каждый участник соединяется с
    /// каждым — голос идёт напрямую, без сервера).
    RoomMembers {
        sender_id: u64,
        room_id: u32,
        /// NodeID участников, не считая получателя.
        members: Vec<String>,
    },
    /// Общий чат комнаты: сообщение видят все участники эфира, а не один друг.
    ///
    /// `room_id = 0` — общий эфир без созвона (комната собрана одним mesh).
    /// Ник едет в самом пакете: у получателя нет `config.ini` отправителя, а
    /// показывать вместо имени хеш NodeID не хочется.
    RoomChat {
        sender_id: u64,
        room_id: u32,
        nick: String,
        text: String,
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
            video_gen,
        } => {
            data.push(5);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.push(if *is_host { 1 } else { 0 });
            data.push(*hops);
            let node_bytes = node_id.as_bytes();
            data.extend_from_slice(&(node_bytes.len() as u16).to_le_bytes());
            data.extend_from_slice(node_bytes);
            // Поколение трансляции — в конец: старые парсеры читают только
            // NodeID и лишние байты игнорируют, новый читает 0, если их нет.
            data.extend_from_slice(&video_gen.to_le_bytes());
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
            room_id,
            nick,
        } => {
            data.push(10);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
            push_str(&mut data, nick);
        }
        NetworkMessage::CallAccept { sender_id, room_id } => {
            data.push(11);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
        }
        NetworkMessage::CallDecline {
            sender_id,
            room_id,
            reason,
        } => {
            data.push(12);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
            data.push(*reason);
        }
        NetworkMessage::CallCancel { sender_id, room_id } => {
            data.push(13);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
        }
        NetworkMessage::CallEnd { sender_id, room_id } => {
            data.push(14);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
        }
        NetworkMessage::PresencePing { sender_id } => {
            data.push(15);
            data.extend_from_slice(&sender_id.to_le_bytes());
        }
        NetworkMessage::PresencePong { sender_id } => {
            data.push(16);
            data.extend_from_slice(&sender_id.to_le_bytes());
        }
        NetworkMessage::RoomMembers {
            sender_id,
            room_id,
            members,
        } => {
            data.push(17);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
            data.extend_from_slice(&(members.len() as u16).to_le_bytes());
            for member in members {
                push_str(&mut data, member);
            }
        }
        NetworkMessage::RoomChat {
            sender_id,
            room_id,
            nick,
            text,
        } => {
            data.push(18);
            data.extend_from_slice(&sender_id.to_le_bytes());
            data.extend_from_slice(&room_id.to_le_bytes());
            push_str(&mut data, nick);
            push_str(&mut data, text);
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
                // Пакет старой версии кончается на NodeID — поколения нет.
                let video_gen = if bytes.len() >= 17 + len {
                    u32::from_le_bytes(bytes[13 + len..17 + len].try_into().ok()?)
                } else {
                    0
                };
                Some(NetworkMessage::IrohSync {
                    sender_id,
                    node_id,
                    is_host,
                    hops,
                    video_gen,
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
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            let mut off = 13;
            let nick = read_str(bytes, &mut off)?;
            Some(NetworkMessage::CallInvite {
                sender_id,
                room_id,
                nick,
            })
        }
        11 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallAccept { sender_id, room_id })
        }
        12 if bytes.len() >= 14 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallDecline {
                sender_id,
                room_id,
                reason: bytes[13],
            })
        }
        13 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallCancel { sender_id, room_id })
        }
        14 if bytes.len() >= 13 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            Some(NetworkMessage::CallEnd { sender_id, room_id })
        }
        15 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::PresencePing { sender_id })
        }
        16 if bytes.len() >= 9 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            Some(NetworkMessage::PresencePong { sender_id })
        }
        17 if bytes.len() >= 15 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            let count = u16::from_le_bytes([bytes[13], bytes[14]]) as usize;
            // Больше 64 участников не бывает: пакет с таким числом — мусор.
            if count > 64 {
                return None;
            }
            let mut off = 15;
            let mut members = Vec::with_capacity(count);
            for _ in 0..count {
                members.push(read_str(bytes, &mut off)?);
            }
            Some(NetworkMessage::RoomMembers {
                sender_id,
                room_id,
                members,
            })
        }
        // 1 байт тега + 8 sender + 4 room + две строки (минимум по 2 байта).
        18 if bytes.len() >= 17 => {
            let sender_id = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
            let room_id = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
            let mut off = 13;
            let nick = read_str(bytes, &mut off)?;
            let text = read_str(bytes, &mut off)?;
            Some(NetworkMessage::RoomChat {
                sender_id,
                room_id,
                nick,
                text,
            })
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
            video_gen: 0x1234_5678,
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
            room_id: 0xDEAD_BEEF,
            nick: "Костя".into(),
        });
        roundtrip(NetworkMessage::CallAccept {
            sender_id: 6,
            room_id: 9,
        });
        roundtrip(NetworkMessage::CallDecline {
            sender_id: 7,
            room_id: 10,
            reason: 2,
        });
        roundtrip(NetworkMessage::CallCancel {
            sender_id: 8,
            room_id: 11,
        });
        roundtrip(NetworkMessage::CallEnd {
            sender_id: 9,
            room_id: 12,
        });
        roundtrip(NetworkMessage::PresencePing { sender_id: 10 });
        roundtrip(NetworkMessage::PresencePong {
            sender_id: 0xFFFF_FFFF_FFFF_FFFF,
        });
        roundtrip(NetworkMessage::RoomMembers {
            sender_id: 12,
            room_id: 0x0BAD_F00D,
            members: vec!["ab".repeat(32), "cd".repeat(32)],
        });
        roundtrip(NetworkMessage::RoomChat {
            sender_id: 13,
            room_id: 0,
            nick: "Узел 2".into(),
            text: "привет, эфир".into(),
        });
    }

    #[test]
    fn room_chat_layout_and_empty_text() {
        let bytes = serialize_msg(&NetworkMessage::RoomChat {
            sender_id: 4,
            room_id: 0x0102_0304,
            nick: "Я".into(),
            text: "ок".into(),
        });
        assert_eq!(bytes[0], 18);
        assert_eq!(u32::from_le_bytes(bytes[9..13].try_into().unwrap()), 0x0102_0304);
        match deserialize_msg(&bytes) {
            Some(NetworkMessage::RoomChat { sender_id, room_id, nick, text }) => {
                assert_eq!(sender_id, 4);
                assert_eq!(room_id, 0x0102_0304);
                assert_eq!(nick, "Я");
                assert_eq!(text, "ок");
            }
            other => panic!("RoomChat разобран неверно: {other:?}"),
        }
        // обрезанный пакет (нет второй строки) — мусор, а не сообщение
        assert!(deserialize_msg(&bytes[..bytes.len() - 1]).is_none());
        // пустой текст кодируется честно: длина 0
        let empty = serialize_msg(&NetworkMessage::RoomChat {
            sender_id: 4,
            room_id: 0,
            nick: "Я".into(),
            text: String::new(),
        });
        assert!(matches!(
            deserialize_msg(&empty),
            Some(NetworkMessage::RoomChat { text, .. }) if text.is_empty()
        ));
    }

    #[test]
    fn room_members_layout_and_truncation() {
        let bytes = serialize_msg(&NetworkMessage::RoomMembers {
            sender_id: 1,
            room_id: 7,
            members: vec!["aa".into(), "bb".into()],
        });
        assert_eq!(&bytes[..1], &[17]);
        assert_eq!(u32::from_le_bytes(bytes[9..13].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes([bytes[13], bytes[14]]), 2);
        assert!(deserialize_msg(&bytes).is_some());
        // обрезанный список не должен превращаться в комнату без участников
        assert!(deserialize_msg(&bytes[..bytes.len() - 1]).is_none());
        assert!(deserialize_msg(&bytes[..14]).is_none());
    }

    #[test]
    fn presence_needs_9_bytes() {
        let ping = serialize_msg(&NetworkMessage::PresencePing { sender_id: 5 });
        assert_eq!(ping.len(), 9);
        assert_eq!(&ping[..1], &[15]);
        assert!(deserialize_msg(&ping[..8]).is_none());
        let pong = serialize_msg(&NetworkMessage::PresencePong { sender_id: 5 });
        assert_eq!(&pong[..1], &[16]);
        assert!(deserialize_msg(&pong[..8]).is_none());
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
            video_gen: 7,
        });
        assert_eq!(&bytes[..1], &[5]);
        assert_eq!(bytes[9], 1); // is_host
        assert_eq!(bytes[10], 1); // hops
        assert_eq!(u16::from_le_bytes([bytes[11], bytes[12]]), 4);
        assert_eq!(&bytes[13..17], b"node");
        assert_eq!(u32::from_le_bytes(bytes[17..21].try_into().unwrap()), 7);

        // обрезанный node_id — отказ, а не пакет с пустым NodeID
        assert!(deserialize_msg(&bytes[..15]).is_none());
        let mut cut = bytes.clone();
        cut.truncate(13);
        assert!(deserialize_msg(&cut).is_none());
    }

    #[test]
    fn iroh_sync_old_format_has_no_video() {
        // Пакет предыдущей версии: всё то же, но без video_gen в конце.
        let mut bytes = serialize_msg(&NetworkMessage::IrohSync {
            sender_id: 0xAA,
            node_id: "node".into(),
            is_host: false,
            hops: 0,
            video_gen: 9,
        });
        bytes.truncate(17);
        match deserialize_msg(&bytes) {
            Some(NetworkMessage::IrohSync { video_gen, .. }) => {
                assert_eq!(video_gen, 0, "старый пакет не должен объявлять трансляцию")
            }
            other => panic!("старый IrohSync разобран неверно: {other:?}"),
        }
    }

    #[test]
    fn truncated_calls_rejected() {
        let accept = serialize_msg(&NetworkMessage::CallAccept {
            sender_id: 1,
            room_id: 2,
        });
        assert!(deserialize_msg(&accept[..12]).is_none());

        let decline = serialize_msg(&NetworkMessage::CallDecline {
            sender_id: 1,
            room_id: 2,
            reason: 1,
        });
        assert!(deserialize_msg(&decline[..13]).is_none());

        let invite = serialize_msg(&NetworkMessage::CallInvite {
            sender_id: 1,
            room_id: 2,
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


