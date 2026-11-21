//! Сессионная история текстового чата (DM). Только в памяти — на диск не пишется.

use std::collections::HashMap;
use std::sync::Mutex;

/// Одно сообщение в переписке.
#[derive(Clone)]
pub struct ChatMessage {
    pub from_me: bool,
    pub text: String,
    pub at: u64,
}

struct Thread {
    messages: Vec<ChatMessage>,
    unread: usize,
}

/// История DM, ключ — NodeID собеседника. Не персистится.
pub struct ChatLog {
    threads: Mutex<HashMap<String, Thread>>,
}

const MAX_THREAD: usize = 200;

impl ChatLog {
    pub fn new() -> Self {
        ChatLog {
            threads: Mutex::new(HashMap::new()),
        }
    }

    pub fn push(&self, node_id: &str, msg: ChatMessage) {
        let mut threads = self.threads.lock().unwrap();
        let t = threads.entry(node_id.to_string()).or_insert(Thread {
            messages: Vec::new(),
            unread: 0,
        });
        if !msg.from_me {
            t.unread += 1;
        }
        t.messages.push(msg);
        while t.messages.len() > MAX_THREAD {
            t.messages.remove(0);
        }
    }

    pub fn thread(&self, node_id: &str) -> Vec<ChatMessage> {
        self.threads
            .lock()
            .unwrap()
            .get(node_id)
            .map(|t| t.messages.clone())
            .unwrap_or_default()
    }

    pub fn unread(&self, node_id: &str) -> usize {
        self.threads
            .lock()
            .unwrap()
            .get(node_id)
            .map(|t| t.unread)
            .unwrap_or(0)
    }

    pub fn mark_read(&self, node_id: &str) {
        if let Some(t) = self.threads.lock().unwrap().get_mut(node_id) {
            t.unread = 0;
        }
    }
}

/// Сообщение общего чата комнаты: в отличие от DM, видно всем в эфире.
#[derive(Clone)]
pub struct RoomMessage {
    pub from_me: bool,
    pub sender_id: u64,
    pub nick: String,
    pub text: String,
    pub at: u64,
}

struct RoomState {
    room_id: u32,
    messages: Vec<RoomMessage>,
    unread: usize,
}

/// История общего чата комнаты. Тоже только в памяти: на диск не пишется.
///
/// Комната в приложении одна: общий эфир (`room_id = 0`) или созвон с
/// приглашением. При смене комнаты история стирается — иначе в новом эфире
/// висели бы сообщения прошлого разговора.
pub struct RoomChat {
    state: Mutex<RoomState>,
}

const MAX_ROOM: usize = 200;

impl RoomChat {
    pub fn new() -> Self {
        RoomChat {
            state: Mutex::new(RoomState {
                room_id: 0,
                messages: Vec::new(),
                unread: 0,
            }),
        }
    }

    pub fn push(&self, room_id: u32, msg: RoomMessage) {
        let mut st = self.state.lock().unwrap();
        if st.room_id != room_id {
            st.room_id = room_id;
            st.messages.clear();
            st.unread = 0;
        }
        if !msg.from_me {
            st.unread += 1;
        }
        st.messages.push(msg);
        while st.messages.len() > MAX_ROOM {
            st.messages.remove(0);
        }
    }

    pub fn messages(&self) -> Vec<RoomMessage> {
        self.state.lock().unwrap().messages.clone()
    }

    pub fn unread(&self) -> usize {
        self.state.lock().unwrap().unread
    }

    pub fn mark_read(&self) {
        self.state.lock().unwrap().unread = 0;
    }

    pub fn room_id(&self) -> u32 {
        self.state.lock().unwrap().room_id
    }
}

impl Default for RoomChat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod room_tests {
    use super::*;

    fn msg(from_me: bool, text: &str) -> RoomMessage {
        RoomMessage {
            from_me,
            sender_id: 7,
            nick: "Узел".into(),
            text: text.into(),
            at: 1,
        }
    }

    #[test]
    fn room_change_clears_history() {
        let chat = RoomChat::new();
        chat.push(42, msg(false, "в комнате 42"));
        assert_eq!(chat.messages().len(), 1);
        chat.push(43, msg(false, "в комнате 43"));
        assert_eq!(chat.messages().len(), 1);
        assert_eq!(chat.messages()[0].text, "в комнате 43");
        assert_eq!(chat.room_id(), 43);
    }

    #[test]
    fn unread_counts_only_incoming() {
        let chat = RoomChat::new();
        chat.push(0, msg(true, "своё"));
        assert_eq!(chat.unread(), 0);
        chat.push(0, msg(false, "чужое"));
        chat.push(0, msg(false, "ещё"));
        assert_eq!(chat.unread(), 2);
        chat.mark_read();
        assert_eq!(chat.unread(), 0);
        assert_eq!(chat.messages().len(), 3);
    }

    #[test]
    fn history_is_capped() {
        let chat = RoomChat::new();
        for i in 0..(MAX_ROOM + 10) {
            chat.push(0, msg(false, &i.to_string()));
        }
        assert_eq!(chat.messages().len(), MAX_ROOM);
        assert_eq!(chat.messages()[MAX_ROOM - 1].text, (MAX_ROOM + 9).to_string());
    }
}
