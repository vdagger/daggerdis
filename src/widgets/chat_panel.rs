//! DM-чат: выдвижная панель с сессионной перепиской

use crate::chat::{ChatLog, ChatMessage};
use crate::friends::FriendsManager;
use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use eframe::egui;
use egui::{Align, Color32, Layout, RichText, Rounding, Stroke};
use std::sync::Arc;

use super::room_panel::short_node_id;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
        // send_bytes сам подключается, если соединения нет: раньше здесь
        // вдобавок шёл connect_to_peer, и при перезапуске пира два соединения
        // гонялись наперегонки за одну запись в карту.
        let _ = p2p.send_bytes(&target, &bytes).await;
    });
}

fn bubble(ui: &mut egui::Ui, m: &ChatMessage, time: &str) {
    let outgoing = m.from_me;
    ui.with_layout(
        if outgoing {
            Layout::right_to_left(Align::TOP)
        } else {
            Layout::left_to_right(Align::TOP)
        },
        |ui| {
            let max_w = ui.available_width() * 0.82;
            let (fill, text_color, border) = if outgoing {
                (t::BRAND_2, Color32::WHITE, t::with_alpha(t::BRAND, 0.0))
            } else {
                (t::SURFACE_HI, t::TEXT, t::STROKE_SOFT)
            };
            egui::Frame::none()
                .fill(fill)
                .rounding(Rounding::same(12.0))
                .stroke(Stroke::new(1.0_f32, border))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .show(ui, |ui| {
                    ui.set_max_width(max_w);
                    ui.label(RichText::new(m.text.as_str()).size(11.5).color(text_color));
                    ui.add_space(1.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(time)
                                .size(8.5)
                                .color(t::with_alpha(text_color, 0.55)),
                        );
                    });
                });
        },
    );
    ui.add_space(6.0);
}

/// Смещение местного времени от UTC в секундах. Раньше часы были жёстко
/// привязаны к UTC+3 — в другом поясе все сообщения показывались со сдвигом.
fn local_offset_seconds() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(windows)]
        {
            // В windows 0.62 `GetLocalTime` живёт в SystemInformation и
            // возвращает SYSTEMTIME напрямую; UTC берём из std.
            use windows::Win32::Foundation::SYSTEMTIME;
            use windows::Win32::System::SystemInformation::GetLocalTime;
            fn to_secs(st: &SYSTEMTIME) -> i64 {
                fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
                    let y = if m <= 2 { y - 1 } else { y };
                    let era = if y >= 0 { y } else { y - 399 } / 400;
                    let yoe = y - era * 400;
                    let mp = (m + 9) % 12;
                    let doy = (153 * mp + 2) / 5 + d - 1;
                    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
                    era * 146_097 + doe - 719_468
                }
                days_from_civil(st.wYear as i64, st.wMonth as i64, st.wDay as i64) * 86_400
                    + st.wHour as i64 * 3600
                    + st.wMinute as i64 * 60
                    + st.wSecond as i64
            }
            unsafe {
                let local = GetLocalTime();
                let utc = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                to_secs(&local) - utc
            }
        }
        #[cfg(not(windows))]
        {
            0
        }
    })
}

fn clock(at_ms: u64) -> String {
    let secs = (at_ms as i64 / 1000 + local_offset_seconds()).rem_euclid(86_400);
    let minutes = (secs / 60) % 60;
    let hours = secs / 3600;
    format!("{:02}:{:02}", hours, minutes)
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

    // Шапка: друг и состояние
    ui.horizontal(|ui| {
        t::avatar(ui, &name, 30.0, false);
        ui.vertical(|ui| {
            ui.label(RichText::new(name.as_str()).size(13.0).strong().color(t::TEXT));
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 2.5, t::LIVE);
                ui.label(
                    RichText::new(short_node_id(node_id))
                        .monospace()
                        .size(9.0)
                        .color(t::TEXT_MUTED),
                );
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if t::icon_button(ui, t::Icon::Close, 26.0, false)
                .on_hover_text("Закрыть")
                .clicked()
            {
                *close = true;
            }
        });
    });

    ui.add_space(6.0);
    t::divider(ui);
    ui.add_space(2.0);

    let msgs = chat.thread(node_id);
    let footer_h = 22.0;
    let composer_h = 46.0;
    let scroll_h = (ui.available_height() - composer_h - footer_h - 12.0).max(80.0);

    egui::ScrollArea::vertical()
        .max_height(scroll_h)
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if msgs.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Переписка пуста. Напишите первым.")
                        .size(10.5)
                        .color(t::TEXT_MUTED),
                );
            } else {
                for m in &msgs {
                    bubble(ui, m, &clock(m.at));
                }
            }
        });

    ui.add_space(4.0);

    // Ввод
    let mut send = false;
    ui.horizontal(|ui| {
        let btn = 30.0;
        let width = (ui.available_width() - btn - 8.0).max(80.0);
        egui::Frame::none()
            .fill(t::INSET)
            .rounding(Rounding::same(9.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(9.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(width - 18.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(input)
                        .hint_text("Сообщение…")
                        .char_limit(500)
                        .desired_width(width - 18.0)
                        .frame(false),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    send = true;
                }
            });

        // Круглая кнопка отправки
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(btn, 30.0), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.painter().circle_filled(
            rect.center(),
            15.0,
            if response.hovered() {
                t::BRAND
            } else {
                t::BRAND_2
            },
        );
        t::draw_icon(
            &ui.painter_at(rect),
            egui::Rect::from_center_size(rect.center(), egui::vec2(13.0, 13.0)),
            t::Icon::Send,
            Color32::WHITE,
        );
        if response.clicked() {
            send = true;
        }
    });

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

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Только друзья · сквозное шифрование")
                .size(8.0)
                .color(t::with_alpha(t::TEXT_MUTED, 0.8)),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new("ИСТОРИЯ ДО ЗАКРЫТИЯ")
                    .size(8.0)
                    .color(t::with_alpha(t::TEXT_MUTED, 0.8)),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{}/500", input.chars().count()))
                    .size(8.5)
                    .monospace()
                    .color(t::TEXT_MUTED),
            );
        });
    });
}
