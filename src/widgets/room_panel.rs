//! Участники эфира и подключение к комнате

use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use eframe::egui;
use egui::{Color32, Rounding, Stroke};
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Короткая форма NodeID: начало и конец
pub fn short_node_id(node_id: &str) -> String {
    if node_id.len() > 18 {
        format!("{}…{}", &node_id[..8], &node_id[node_id.len() - 7..])
    } else {
        node_id.to_string()
    }
}

fn status_text(speaking: bool, alive: bool) -> (&'static str, Color32) {
    if speaking {
        ("говорит", t::LIVE)
    } else if alive {
        ("молчит", t::TEXT_MUTED)
    } else {
        ("нет связи", t::REC)
    }
}

/// Тонкая линия между строками списка
fn row_divider(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, t::with_alpha(t::STROKE_SOFT, 0.7));
    ui.add_space(2.0);
}

pub fn draw_participants(
    ui: &mut egui::Ui,
    room: std::sync::Arc<crate::room::RoomManager>,
    mixer: &crate::mixer::AudioMixer,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    is_transmitting: bool,
    nick: &str,
    friends: &std::sync::Arc<crate::friends::FriendsManager>,
) {
    let now = now_ms();
    let peers = room.get_all_peers();
    let my_is_host = is_host.load(Ordering::Relaxed);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Участники")
                .size(14.0)
                .strong()
                .color(t::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{}", peers.len() + 1))
                    .size(10.0)
                    .monospace()
                    .color(t::TEXT_MUTED),
            );
        });
    });
    ui.add_space(2.0);

    // Себя — плоская строка, как в макете
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("01")
                .monospace()
                .size(8.5)
                .color(t::TEXT_MUTED),
        );
        ui.add_space(6.0);
        t::avatar(ui, nick, 32.0, is_transmitting);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Вы")
                    .size(12.0)
                    .strong()
                    .color(t::TEXT),
            );
            // Как в макете: «ХОСТ · молчит» одной строкой статуса
            let (status, color) = status_text(is_transmitting, true);
            let (line, color) = if my_is_host {
                (format!("ХОСТ · {}", status), t::with_alpha(t::GOLD, 0.85))
            } else {
                (status.to_string(), color)
            };
            ui.label(egui::RichText::new(line).size(9.5).color(color));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (bars, _) = ui.allocate_exact_size(egui::vec2(16.0, 14.0), egui::Sense::hover());
            t::level_bars(
                &ui.painter_at(bars),
                bars,
                if is_transmitting { 0.8 } else { 0.0 },
                if is_transmitting { t::LIVE } else { t::TEXT_MUTED },
            );
        });
    });
    ui.add_space(2.0);

    // Имена друзей известны локально (friends.json) — показываем их вместо
    // «Гость 3f2a». Ключ — NodeID участника, а не FNV-хеш `peer.id`.
    let known_friends = friends.friends();

    for (idx, peer) in peers.into_iter().enumerate() {
        let alive = now.saturating_sub(peer.last_heartbeat) < 3000;
        let speaking = mixer.is_peer_speaking(peer.id);
        let level = mixer.peer_meter(peer.id).peak;

        row_divider(ui);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{:02}", idx + 2))
                    .monospace()
                    .size(8.5)
                    .color(t::TEXT_MUTED),
            );
            ui.add_space(6.0);
            let id_str = peer.id.to_string();
            let short_id = &id_str[..4.min(id_str.len())];
            let name = known_friends
                .iter()
                .find(|f| f.node_id == peer.address)
                .map(|f| f.name.clone())
                .unwrap_or_else(|| format!("Гость {}", short_id));
            t::avatar(ui, &name, 32.0, speaking);
            ui.add_space(8.0);

            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(name.as_str())
                        .size(12.0)
                        .strong()
                        .color(t::TEXT),
                );
                let (status, color) = status_text(speaking, alive);
                ui.label(egui::RichText::new(status).size(9.5).color(color));
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Динамик (справа) открывает громкость собеседника
                let speaker = t::icon_button(ui, t::Icon::Speaker, 20.0, false)
                    .on_hover_text("Громкость собеседника");
                let popup_id = ui.make_persistent_id(("peer_vol", peer.id));
                if speaker.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup_id));
                }
                egui::popup_below_widget(
                    ui,
                    popup_id,
                    &speaker,
                    // CloseOnClick закрывал попап от любого клика, включая
                    // клик по самому слайдеру: тянуть его было невозможно.
                    egui::popup::PopupCloseBehavior::CloseOnClickOutside,
                    |ui| {
                        ui.set_min_width(170.0);
                        ui.spacing_mut().slider_width = 110.0;
                        // Шкала в дБ: −30 .. +18, шаг 0.5 — прибавлять/убавлять
                        // человека на понятное число децибел, а не на проценты.
                        let mut db = crate::mixer::slider_to_db(mixer.get_peer_volume(peer.id));
                        if ui
                            .add(
                                egui::Slider::new(&mut db, -30.0..=18.0)
                                    .step_by(0.5)
                                    .suffix(" дБ")
                                    .fixed_decimals(1),
                            )
                            .changed()
                        {
                            mixer.set_peer_volume(peer.id, crate::mixer::db_to_slider(db));
                        }
                    },
                );

                let (bars, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 14.0), egui::Sense::hover());
                t::level_bars(
                    &ui.painter_at(bars),
                    bars,
                    level,
                    if speaking { t::LIVE } else { t::TEXT_MUTED },
                );
            });
        });
        ui.add_space(2.0);
    }
}

/// Комната: вход по NodeID. Возвращает true, если пользователь попросил выйти
/// из эфира (тогда gui закрывает звонок и рвёт соединения).
pub fn draw_room_connect(
    ui: &mut egui::Ui,
    connect_ip: &mut String,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    room: std::sync::Arc<crate::room::RoomManager>,
    calls: &Arc<crate::calls::CallManager>,
) -> bool {
    let mut leave = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Комната")
                .size(14.0)
                .strong()
                .color(t::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            t::micro(ui, "P2P");
        });
    });

    ui.add_space(6.0);

    ui.horizontal(|ui| {
        let btn_width = 72.0;
        let spacing = ui.spacing().item_spacing.x;
        let input_width = (ui.available_width() - btn_width - spacing).max(80.0);

        egui::Frame::none()
            .fill(t::INSET)
            .rounding(Rounding::same(10.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("NODE ID")
                            .size(7.5)
                            .color(t::TEXT_MUTED),
                    );
                    ui.add(
                        egui::TextEdit::singleline(connect_ip)
                            .hint_text("NodeID собеседника")
                            .desired_width(input_width - 20.0)
                            .frame(false),
                    );
                });
            });

        if t::primary_button(ui, "Войти", egui::vec2(btn_width, 44.0)).clicked() {
            // room.join() зовётся внутри join_room — только после проверки NodeID
            join_room(connect_ip, iroh.clone(), my_id, is_host.clone(), room.clone(), calls);
        }
    });

    // Выход из эфира: без него из активного звонка выбирались только закрытием
    // приложения, а завершённый звонок через секунду «возобновлялся» сам.
    if !room.get_all_peers().is_empty() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let width = ui.available_width();
            if ui
                .add_sized(
                    egui::vec2(width, 28.0),
                    egui::Button::new(
                        egui::RichText::new("Выйти из эфира")
                            .size(10.5)
                            .strong()
                            .color(t::REC),
                    )
                    .fill(t::with_alpha(t::REC, 0.10))
                    .rounding(Rounding::same(8.0))
                    .stroke(Stroke::new(1.0_f32, t::with_alpha(t::REC, 0.35))),
                )
                .clicked()
            {
                leave = true;
            }
        });
    }

    let peers = room.get_all_peers();
    if !is_host.load(Ordering::Relaxed) {
        ui.add_space(4.0);
        let host_addr = peers
            .iter()
            .find(|p| p.is_host)
            .map(|p| short_node_id(&p.address))
            .unwrap_or_else(|| "Ожидание хоста…".to_string());
        ui.label(
            egui::RichText::new(format!("Хост: {}", host_addr))
                .size(9.5)
                .color(t::TEXT_MUTED),
        );
    }

    leave
}

/// Свой NodeID и копирование — одной строкой, как в макете.
/// Возвращает true, если NodeID только что скопирован.
pub fn draw_my_nodeid(ui: &mut egui::Ui, my_node_id: &str) -> bool {
    let mut copied = false;

    ui.horizontal(|ui| {
        t::micro(ui, "МОЙ NODEID");
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(short_node_id(my_node_id))
                .monospace()
                .size(9.5)
                .color(t::TEXT_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (chip, resp) = ui.allocate_exact_size(egui::vec2(96.0, 24.0), egui::Sense::click());
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Скопировать и отправить другу");
            let painter = ui.painter_at(chip);
            painter.rect_filled(
                chip,
                Rounding::same(6.0),
                if resp.hovered() {
                    egui::Color32::from_rgb(34, 34, 40)
                } else {
                    egui::Color32::from_rgb(28, 28, 34)
                },
            );
            painter.rect_stroke(
                chip,
                Rounding::same(6.0),
                Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.22)),
            );
            t::draw_icon(
                &painter,
                egui::Rect::from_center_size(
                    egui::pos2(chip.left() + 14.0, chip.center().y),
                    egui::vec2(11.0, 11.0),
                ),
                t::Icon::Copy,
                t::BRAND,
            );
            painter.text(
                egui::pos2(chip.left() + 25.0, chip.center().y),
                egui::Align2::LEFT_CENTER,
                "Копировать",
                egui::FontId::proportional(9.5),
                t::BRAND,
            );
            if resp.clicked() {
                ui.output_mut(|o| o.copied_text = my_node_id.to_string());
                copied = true;
            }
        });
    });

    copied
}

pub(crate) fn join_room(
    connect_ip: &mut String,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    room: std::sync::Arc<crate::room::RoomManager>,
    calls: &Arc<crate::calls::CallManager>,
) {
    let raw_addr = connect_ip.trim();

    if raw_addr.len() != 64 || !raw_addr.chars().all(|c| c.is_ascii_hexdigit()) {
        eprintln!("[ОШИБКА] NodeID должен состоять из 64 hex-символов.");
        return;
    }

    let Some(ref p2p) = iroh else {
        return;
    };

    let p2p_clone = p2p.clone();
    let target_node_id = raw_addr.to_string();
    let my_peer_id = my_id;

    // Явный вход: возвращаемся в эфир и снимаем блокировку, оставшуюся после
    // прошлого завершённого звонка.
    room.join();
    calls.unblock_peer(&target_node_id);
    is_host.store(false, Ordering::SeqCst);
    println!(
        "[СЕТЬ] Подключение к узлу (Iroh P2P). Цель: {}",
        target_node_id
    );

    tokio::task::spawn(async move {
        println!("[IROH] Попытка P2P-подключения к NodeID: {}", target_node_id);
        match p2p_clone.connect_to_peer(&target_node_id).await {
            Ok(()) => {
                println!("[IROH] Прямое P2P-подключение установлено");
                let hello_msg = crate::network::NetworkMessage::Heartbeat {
                    sender_id: my_peer_id,
                    is_host: false,
                };
                let bytes = crate::network::message::serialize_msg(&hello_msg);
                p2p_clone.broadcast_bytes(&bytes).await;
            }
            Err(e) => eprintln!("[IROH] Ошибка P2P-подключения по NodeID: {}", e),
        }
    });
}
