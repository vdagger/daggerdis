//! DM-чат: выдвижная панель с сессионной перепиской

use crate::chat::{ChatLog, ChatMessage, RoomChat};
use crate::friends::FriendsManager;
use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::room::RoomManager;
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

fn send_chat(
    p2p: &Arc<IrohP2pManager>,
    target: &str,
    my_id: u64,
    text: String,
    error_slot: Arc<std::sync::Mutex<Option<String>>>,
) {
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
        // Ошибку не глотаем: сообщение уже нарисовано в ленте, и человек
        // уверен, что оно ушло, — тост говорит обратное.
        if let Err(e) = p2p.send_bytes(&target, &bytes).await {
            if let Ok(mut slot) = error_slot.lock() {
                *slot = Some(format!("Сообщение не доставлено: {}", e));
            }
        }
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
                    // Текст и время — один LabelJob. Вложенная раскладка
                    // (`with_layout(right_to_left)` внутри рамки) растягивала
                    // пузырь на всю панель: рамка считала своей ширину
                    // доступного места, а не текста.
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        &m.text,
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(11.5),
                            color: text_color,
                            ..Default::default()
                        },
                    );
                    job.append(
                        &format!("  {time}"),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(8.5),
                            color: t::with_alpha(text_color, 0.55),
                            ..Default::default()
                        },
                    );
                    ui.label(job);
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
    error_slot: &Arc<std::sync::Mutex<Option<String>>>,
    presence: &Arc<crate::presence::PresenceTracker>,
) {
    let name = friend_name(friends, node_id);
    let online = presence.is_online(node_id);

    // Шапка: друг и состояние
    ui.horizontal(|ui| {
        t::avatar(ui, &name, 30.0, false);
        ui.vertical(|ui| {
            ui.label(RichText::new(name.as_str()).size(13.0).strong().color(t::TEXT));
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                // Точка — настоящий онлайн-статус из presence, а не декоративная.
                ui.painter().circle_filled(
                    dot.center(),
                    2.5,
                    if online { t::LIVE } else { t::TEXT_MUTED },
                );
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
                send_chat(p2p, node_id, my_id, text, error_slot.clone());
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

/// Отправка в общий чат: одно сообщение всем участникам комнаты.
///
/// Датаграммы уходят параллельно одному и тому же набору адресов, что и голос,
/// поэтому «написать в эфир» не требует отдельного соединения. Ошибка видна
/// тостом: сообщение уже нарисовано в ленте, и человек уверен, что оно ушло.
fn send_room_chat(
    p2p: &Arc<IrohP2pManager>,
    targets: Vec<String>,
    room_id: u32,
    my_id: u64,
    nick: String,
    text: String,
    error_slot: Arc<std::sync::Mutex<Option<String>>>,
) {
    let text = truncate_to_bytes(text, 1024);
    let bytes =
        crate::network::message::serialize_msg(&crate::network::NetworkMessage::RoomChat {
            sender_id: my_id,
            room_id,
            nick,
            text,
        });
    if targets.is_empty() {
        return;
    }
    let p2p = p2p.clone();
    tokio::task::spawn(async move {
        let mut failed = 0usize;
        for target in targets {
            if p2p.send_bytes(&target, &bytes).await.is_err() {
                failed += 1;
            }
        }
        if failed > 0 {
            if let Ok(mut slot) = error_slot.lock() {
                *slot = Some(format!(
                    "Сообщение в комнату не доставлено: {failed} участн."
                ));
            }
        }
    });
}

/// Общий чат комнаты: видно всем, кто в эфире, в отличие от лички другу.
#[allow(clippy::too_many_arguments)]
pub fn draw_room_chat_panel(
    ui: &mut egui::Ui,
    room_chat: &Arc<RoomChat>,
    my_nick: &str,
    my_id: u64,
    room: &Arc<RoomManager>,
    room_id: u32,
    iroh: Option<Arc<IrohP2pManager>>,
    input: &mut String,
    close: &mut bool,
    error_slot: &Arc<std::sync::Mutex<Option<String>>>,
) {
    let members = room.get_all_peers().len() + 1;

    ui.horizontal(|ui| {
        t::avatar(ui, "Э", 30.0, true);
        ui.vertical(|ui| {
            ui.label(RichText::new("Комната").size(13.0).strong().color(t::TEXT));
            ui.label(
                RichText::new(format!("{members} участн. · видно всем в эфире"))
                    .size(9.0)
                    .color(t::TEXT_MUTED),
            );
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

    let msgs = room_chat.messages();
    let composer_h = 46.0;
    let scroll_h = (ui.available_height() - composer_h - 30.0).max(80.0);

    egui::ScrollArea::vertical()
        .max_height(scroll_h)
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if msgs.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("В комнате ещё никто не писал. Сообщение увидят все в эфире.")
                        .size(10.5)
                        .color(t::TEXT_MUTED),
                );
            } else {
                for m in &msgs {
                    room_bubble(ui, m, &clock(m.at));
                }
            }
        });

    ui.add_space(4.0);

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
                        .hint_text("Сообщение в комнату…")
                        .char_limit(500)
                        .desired_width(width - 18.0)
                        .frame(false),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    send = true;
                }
            });

        let (rect, response) = ui.allocate_exact_size(egui::vec2(btn, 30.0), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.painter().circle_filled(
            rect.center(),
            15.0,
            if response.hovered() { t::BRAND } else { t::BRAND_2 },
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
                room_chat.push(
                    room_id,
                    crate::chat::RoomMessage {
                        from_me: true,
                        sender_id: my_id,
                        nick: my_nick.to_string(),
                        text: text.clone(),
                        at: now_ms(),
                    },
                );
                send_room_chat(
                    p2p,
                    room.get_all_addresses(),
                    room_id,
                    my_id,
                    my_nick.to_string(),
                    text,
                    error_slot.clone(),
                );
            }
            input.clear();
        }
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new("Только участники эфира · история до закрытия")
            .size(8.0)
            .color(t::with_alpha(t::TEXT_MUTED, 0.8)),
    );
}

/// Пузырь общего чата: у входящего сверху ник, потому что в комнате говорят
/// несколько человек и подпись важнее, чем в личке.
fn room_bubble(ui: &mut egui::Ui, m: &crate::chat::RoomMessage, time: &str) {
    ui.with_layout(
        if m.from_me {
            Layout::right_to_left(Align::TOP)
        } else {
            Layout::left_to_right(Align::TOP)
        },
        |ui| {
            let max_w = ui.available_width() * 0.82;
            let (fill, text_color, border) = if m.from_me {
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
                    // Ник и текст — один LabelJob: ник отдельной строкой над
                    // пузырём не рисовался, а вложенная вертикальная
                    // раскладка растягивала пузырь на всю панель.
                    let mut job = egui::text::LayoutJob::default();
                    if !m.from_me {
                        job.append(
                            &format!("{}\n", m.nick),
                            0.0,
                            egui::TextFormat {
                                font_id: egui::FontId::proportional(8.5),
                                color: t::BRAND,
                                ..Default::default()
                            },
                        );
                    }
                    job.append(
                        &m.text,
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(11.5),
                            color: text_color,
                            ..Default::default()
                        },
                    );
                    job.append(
                        &format!("  {time}"),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(8.5),
                            color: t::with_alpha(text_color, 0.55),
                            ..Default::default()
                        },
                    );
                    ui.label(job);
                    ui.add_space(1.0);
                    // Время — частью того же LabelJob ниже, без вложенной
                    // раскладки: `with_layout(right_to_left)` внутри пузыря
                    // растягивала рамку на всю панель (та же болячка в личке).
                });
        },
    );
    ui.add_space(6.0);
}
