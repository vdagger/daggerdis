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
