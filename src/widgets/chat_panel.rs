//! DM-чат: сессионная переписка с выбранным другом.

use crate::chat::{ChatLog, ChatMessage};
use crate::friends::FriendsManager;
use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use eframe::egui;
use egui::{Align, Color32, Layout, RichText, Rounding, Stroke};
use std::sync::Arc;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn short_node_id(node_id: &str) -> String {
    if node_id.len() > 16 {
        format!("{}…", &node_id[..15])
    } else {
        node_id.to_string()
    }
}

fn friend_name(friends: &Arc<FriendsManager>, node_id: &str) -> String {
    friends
        .friends()
        .iter()
        .find(|f| f.node_id == node_id)
        .map(|f| f.name.clone())
        .unwrap_or_else(|| short_node_id(node_id))
}

fn truncate_to_bytes(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn send_chat(p2p: &Arc<IrohP2pManager>, target: &str, my_id: u64, text: String) {
    let text = truncate_to_bytes(text, 1024);
    let msg = crate::network::NetworkMessage::ChatMessage {
        sender_id: my_id,
        text,
    };
    let bytes = crate::network::message::serialize_msg(&msg);
    let p2p = p2p.clone();
    let target = target.to_string();
    tokio::task::spawn(async move {
        let _ = p2p.connect_to_peer(&target).await;
        let _ = p2p.send_bytes(&target, &bytes).await;
    });
}

fn bubble(ui: &mut egui::Ui, m: &ChatMessage) {
    if m.from_me {
        ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
            bubble_inner(ui, m, t::BRAND, Color32::WHITE);
        });
    } else {
        ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
            bubble_inner(ui, m, t::SURFACE_HI, t::TEXT);
        });
    }
    ui.add_space(3.0);
}

fn bubble_inner(ui: &mut egui::Ui, m: &ChatMessage, fill: Color32, color: Color32) {
    let max_w = ui.available_width() * 0.8;
    egui::Frame::none()
        .fill(fill)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, t::STROKE))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.set_max_width(max_w);
            ui.label(RichText::new(&m.text).size(12.0).color(color));
        });
}

pub fn draw_chat_panel(
    ui: &mut egui::Ui,
    chat: &Arc<ChatLog>,
    friends: &Arc<FriendsManager>,
    node_id: &str,
    input: &mut String,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    close: &mut bool,
) {
    let name = friend_name(friends, node_id);
    let row_width = ui.available_width();

    ui.horizontal(|ui| {
        t::section_label(ui, &format!("ЧАТ · {}", name));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Закрыть").clicked() {
                *close = true;
            }
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    let msgs = chat.thread(node_id);
    if msgs.is_empty() {
        ui.label(
            RichText::new("Переписка пуста. Напишите первым.")
                .size(10.5)
                .color(t::TEXT_MUTED),
        );
    } else {
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for m in &msgs {
                    bubble(ui, m);
                }
            });
    }

    ui.add_space(6.0);

    let send = ui
        .horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(input)
                    .hint_text("Сообщение…")
                    .char_limit(500)
                    .desired_width(row_width - 74.0),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui
                .add_sized(
                    [58.0, 24.0],
                    egui::Button::new(
                        RichText::new("Отправить")
                            .size(10.5)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(t::BRAND_DEEP)
                    .rounding(Rounding::same(8.0)),
                )
                .clicked();
            enter || clicked
        })
        .inner;

    if send {
        let text = input.trim().to_string();
        if !text.is_empty() {
            if let Some(ref p2p) = iroh {
                chat.push(
                    node_id,
                    ChatMessage {
                        from_me: true,
                        text: text.clone(),
                        at: now_ms(),
                    },
                );
                send_chat(p2p, node_id, my_id, text);
            }
            input.clear();
        }
    }
}
