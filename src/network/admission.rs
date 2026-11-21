//! Правила приёма сетевых сообщений — без сети и без состояния.
//!
//! Всё, что раньше было россыпью `if` внутри обработчика iroh, собрано здесь:
//! решения принимаются по явным полям, поэтому их можно проверить тестами, не
//! поднимая ни соединения, ни комнату. Сами обработчики только печатают
//! причину отказа и делают то, что решила функция.

use crate::identity::my_id_from_node_id;

/// Всё, что нужно знать, чтобы решить судьбу `IrohSync`.
pub struct SyncCheck<'a> {
    /// NodeID соединения (QUIC/TLS) — его подделать нельзя.
    pub from: &'a str,
    /// `node_id` из пакета: за кого себя выдаёт отправитель.
    pub node_id: &'a str,
    /// `sender_id` из пакета: хеш NodeID заявленного узла.
    pub sender_id: u64,
    /// 0 — напрямую, 1 — переслан хостом.
    pub hops: u8,
    /// Свой NodeID: себя релеить нельзя.
    pub my_node_id: &'a str,
    /// Отправитель релея состоит в комнате и помечен хостом.
    pub relayed_by_host: bool,
    /// Приглашение ещё не принято — в комнату не пускаем.
    pub blocked: bool,
    /// Мы вышли из эфира сами.
    pub left: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncVerdict {
    Accept,
    /// Больше одного хопа релея.
    RelayTooDeep,
    /// В релее `sender_id` не соответствует узлу, за которого себя выдают.
    RelayForeign,
    /// Хост переслал синхронизацию про нас самих.
    RelaySelf,
    /// Релей не от нашего хоста.
    RelayNotHost,
    /// Прямая синхронизация: `sender_id`/`node_id` не совпадают с соединением.
    Foreign,
    /// Нас пригласили, но мы ещё не решили.
    Blocked,
    /// Мы вне эфира.
    Left,
}

/// Решает судьбу `IrohSync`. Порядок проверок важен: подлинность — раньше
/// состояния комнаты, иначе подделанный пакет мог бы «разбудить» комнату.
pub fn admit_sync(check: &SyncCheck<'_>) -> SyncVerdict {
    if check.hops > 0 {
        if check.hops > 1 {
            return SyncVerdict::RelayTooDeep;
        }
        if my_id_from_node_id(check.node_id) != Some(check.sender_id) {
            return SyncVerdict::RelayForeign;
        }
        if check.node_id == check.my_node_id {
            return SyncVerdict::RelaySelf;
        }
        if !check.relayed_by_host {
            return SyncVerdict::RelayNotHost;
        }
    } else {
        let conn_matches = my_id_from_node_id(check.from) == Some(check.sender_id);
        if !conn_matches || check.node_id != check.from {
            return SyncVerdict::Foreign;
        }
    }
    if check.blocked {
        return SyncVerdict::Blocked;
    }
    if check.left {
        return SyncVerdict::Left;
    }
    SyncVerdict::Accept
}

/// Судьба сообщения общего чата комнаты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatVerdict {
    Accept,
    Foreign,
    NotInAir,
    NotMember,
    WrongRoom,
    EmptyText,
}

/// `room_id` 0 — общий эфир без созвона; иначе номера должны совпадать.
pub fn admit_room_chat(
    sender_verified: bool,
    left: bool,
    member: bool,
    room_id: u32,
    my_room: u32,
    text: &str,
) -> ChatVerdict {
    if !sender_verified {
        return ChatVerdict::Foreign;
    }
    if left {
        return ChatVerdict::NotInAir;
    }
    if !member {
        return ChatVerdict::NotMember;
    }
    if room_id != 0 && my_room != 0 && room_id != my_room {
        return ChatVerdict::WrongRoom;
    }
    if text.trim().is_empty() {
        return ChatVerdict::EmptyText;
    }
    ChatVerdict::Accept
}

/// Список комнаты применяем только к своей комнате: её номер знают лишь
/// приглашённые.
pub fn admit_members(sender_verified: bool, my_room: Option<u32>, room_id: u32) -> bool {
    sender_verified && my_room == Some(room_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Настоящий NodeID из детерминированного ключа: `my_id_from_node_id`
    /// разбирает строку как ed25519-ключ, поэтому выдуманные hex-строки не годятся.
    fn node_id(seed: u8) -> String {
        let mut key = [7u8; 32];
        key[0] = seed;
        iroh::SecretKey::from_bytes(&key).public().to_string()
    }

    fn my_id(node: &str) -> u64 {
        my_id_from_node_id(node).unwrap()
    }

    fn direct<'a>(from: &'a str, node: &'a str) -> SyncCheck<'a> {
        SyncCheck {
            from,
            node_id: node,
            sender_id: my_id(node),
            hops: 0,
            my_node_id: "me",
            relayed_by_host: false,
            blocked: false,
            left: false,
        }
    }

    #[test]
    fn direct_sync_accepts_only_own_connection_identity() {
        let a = node_id(b'a');
        let b = node_id(b'b');
        // соединение и пакет называют один узел
        assert_eq!(admit_sync(&direct(&a, &a)), SyncVerdict::Accept);
        // пакет называет другой узел, чем соединение
        let mut wrong = direct(&a, &b);
        wrong.sender_id = my_id(&a);
        assert_eq!(admit_sync(&wrong), SyncVerdict::Foreign);
        // sender_id не хеш названного NodeID
        let mut bad = direct(&a, &a);
        bad.sender_id = 12345;
        assert_eq!(admit_sync(&bad), SyncVerdict::Foreign);
    }

    #[test]
    fn direct_sync_respects_room_state_after_identity() {
        let a = node_id(b'a');
        let mut left = direct(&a, &a);
        left.left = true;
        assert_eq!(admit_sync(&left), SyncVerdict::Left);
        let mut blocked = direct(&a, &a);
        blocked.blocked = true;
        assert_eq!(admit_sync(&blocked), SyncVerdict::Blocked);
        // и даже с чужим sender_id причина — подделка, а не «вне эфира»
        let mut both = direct(&a, &a);
        both.left = true;
        both.sender_id = 1;
        assert_eq!(admit_sync(&both), SyncVerdict::Foreign);
    }

    #[test]
    fn relayed_sync_requires_one_hop_from_host() {
        let host = node_id(b'h');
        let peer = node_id(b'p');
        let mut check = SyncCheck {
            from: &host,
            node_id: &peer,
            sender_id: my_id(&peer),
            hops: 1,
            my_node_id: "me",
            relayed_by_host: true,
            blocked: false,
            left: false,
        };
        assert_eq!(admit_sync(&check), SyncVerdict::Accept);
        // два хопа — отказ
        check.hops = 2;
        assert_eq!(admit_sync(&check), SyncVerdict::RelayTooDeep);
        check.hops = 1;
        // релей не от хоста
        check.relayed_by_host = false;
        assert_eq!(admit_sync(&check), SyncVerdict::RelayNotHost);
        check.relayed_by_host = true;
        // sender_id не хеш заявленного узла
        check.sender_id = 7;
        assert_eq!(admit_sync(&check), SyncVerdict::RelayForeign);
        // хост переслал про нас самих
        let me = node_id(b'm');
        check.sender_id = my_id(&me);
        check.node_id = &me;
        check.my_node_id = &me;
        assert_eq!(admit_sync(&check), SyncVerdict::RelaySelf);
    }

    #[test]
    fn room_chat_requires_air_membership_and_matching_room() {
        assert_eq!(
            admit_room_chat(true, false, true, 0, 0, "привет"),
            ChatVerdict::Accept
        );
        // пустое сообщение не показываем
        assert_eq!(
            admit_room_chat(true, false, true, 0, 0, "   "),
            ChatVerdict::EmptyText
        );
        // вне эфира
        assert_eq!(
            admit_room_chat(true, true, true, 0, 0, "привет"),
            ChatVerdict::NotInAir
        );
        // не участник комнаты
        assert_eq!(
            admit_room_chat(true, false, false, 0, 0, "привет"),
            ChatVerdict::NotMember
        );
        // другая комната
        assert_eq!(
            admit_room_chat(true, false, true, 5, 6, "привет"),
            ChatVerdict::WrongRoom
        );
        // общий эфир (0) проходит из любой комнаты
        assert_eq!(
            admit_room_chat(true, false, true, 0, 6, "привет"),
            ChatVerdict::Accept
        );
        // от незнакомца
        assert_eq!(
            admit_room_chat(false, false, true, 0, 0, "привет"),
            ChatVerdict::Foreign
        );
    }

    #[test]
    fn members_are_applied_only_to_our_room() {
        assert!(admit_members(true, Some(7), 7));
        assert!(!admit_members(true, Some(7), 8), "чужая комната");
        assert!(!admit_members(true, None, 7), "мы не в комнате");
        assert!(!admit_members(false, Some(7), 7), "чужой sender_id");
    }
}
