//! Список друзей, входящие заявки и добавление нового друга

use crate::chat::ChatLog;
use crate::friends::FriendsManager;
use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use eframe::egui;
use egui::{Color32, Rounding, Stroke};
use std::sync::Arc;

use super::room_panel::short_node_id;

fn send_request(
    p2p: &Arc<IrohP2pManager>,
    friends: &Arc<FriendsManager>,
    target: &str,
    my_id: u64,
    my_node_id: &str,
    nick: &str,
) {
    let req = crate::friends::FriendRequest {
        sender_id: my_id,
        node_id: my_node_id.to_string(),
        nick: nick.to_string(),
    };
    friends.add_outgoing(target.to_string(), req);
    let p2p = p2p.clone();
    let friends = friends.clone();
    let target = target.to_string();
    tokio::task::spawn(async move {
        // send_bytes внутри сам подключается к пиру — отдельный connect только
        // заводил второе соединение наперегонки.
        crate::friends::resend_pending_to(friends.as_ref(), &p2p, &target).await;
    });
}

fn send_accept(
    p2p: &Arc<IrohP2pManager>,
    target: &str,
    my_id: u64,
    my_node_id: &str,
    nick: &str,
) {
    let accept = crate::network::NetworkMessage::FriendAccept {
        sender_id: my_id,
        node_id: my_node_id.to_string(),
        nick: nick.to_string(),
    };
    let p2p = p2p.clone();
    let target = target.to_string();
    tokio::task::spawn(async move {
        let bytes = crate::network::message::serialize_msg(&accept);
        let _ = p2p.send_bytes(&target, &bytes).await;
    });
}

fn send_remove(p2p: &Arc<IrohP2pManager>, target: &str, my_id: u64, my_node_id: &str) {
    let remove = crate::network::NetworkMessage::FriendRemove {
        sender_id: my_id,
        node_id: my_node_id.to_string(),
    };
    let p2p = p2p.clone();
    let target = target.to_string();
    tokio::task::spawn(async move {
        let bytes = crate::network::message::serialize_msg(&remove);
        let _ = p2p.send_bytes(&target, &bytes).await;
    });
}

pub fn draw_friends_panel(
    ui: &mut egui::Ui,
    friends: &Arc<FriendsManager>,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    my_node_id: &str,
    nick: &str,
    add_friend_node: &mut String,
    chat_log: &Arc<ChatLog>,
    active_chat: &mut Option<String>,
    chat_open: &mut bool,
    calls: &Arc<crate::calls::CallManager>,
    presence: &Arc<crate::presence::PresenceTracker>,
    offline_toast: &mut Option<String>,
    room: &Arc<crate::room::RoomManager>,
    is_host: &Arc<std::sync::atomic::AtomicBool>,
) {
    let row_width = ui.available_width();
    let p2p = iroh;
    let incoming = friends.incoming();
    let friend_list = friends.friends();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Друзья")
                .size(14.0)
                .strong()
                .color(t::TEXT),
        );
        if !incoming.is_empty() {
            ui.label(
                egui::RichText::new(format!("{} заявка", incoming.len()))
                    .size(9.5)
                    .color(t::GOLD),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{}", friend_list.len()))
                    .size(10.0)
                    .monospace()
                    .color(t::TEXT_MUTED),
            );
        });
    });

    ui.add_space(5.0);

    // Входящие заявки
    for req in incoming {
        egui::Frame::none()
            .fill(t::with_alpha(t::GOLD, 0.07))
            .rounding(Rounding::same(9.0))
            .stroke(Stroke::new(1.0_f32, t::with_alpha(t::GOLD, 0.20)))
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(row_width - 16.0);
                ui.horizontal(|ui| {
                    t::avatar(ui, &req.nick, 26.0, false);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(req.nick.as_str())
                                .size(11.5)
                                .strong()
                                .color(t::TEXT),
                        );
                        ui.label(
                            egui::RichText::new(format!("заявка · {}", short_node_id(&req.node_id)))
                                .size(9.0)
                                .color(t::TEXT_MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if t::icon_button(ui, t::Icon::Close, 24.0, false)
                            .on_hover_text("Отклонить")
                            .clicked()
                        {
                            friends.decline_incoming(&req.node_id);
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Принять")
                                        .size(9.5)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(t::BRAND_2)
                                .rounding(Rounding::same(6.0)),
                            )
                            .clicked()
                        {
                            let accepted = friends.accept_incoming(&req.node_id);
                            if accepted.is_some() {
                                if let Some(ref p2p) = p2p {
                                    // Роль хоста от дружбы не зависит: сбрасывать
                                    // is_host здесь значило бы ломать failover.
                                    send_accept(p2p, &req.node_id, my_id, my_node_id, nick);
                                }
                            }
                        }
                    });
                });
            });
        ui.add_space(3.0);
    }

    // Список друзей — плоские строки с волосяными линиями, как в макете
    let room_peers = room.get_all_peers();
    for (idx, friend) in friend_list.iter().enumerate() {
        if idx > 0 {
            ui.add_space(2.0);
            let (line, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(line, 0.0, t::with_alpha(t::STROKE_SOFT, 0.7));
            ui.add_space(2.0);
        }

        let online = presence.is_online(&friend.node_id);
        // Друг уже в комнате: это видно по адресу участника (NodeID), и это
        // единственный способ позвать его без вставки NodeID руками.
        let in_room = room_peers.iter().any(|p| p.address == friend.node_id);
        ui.horizontal(|ui| {
            t::avatar(ui, &friend.name, 32.0, false);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&friend.name)
                            .size(12.0)
                            .strong()
                            .color(t::TEXT),
                    );
                    if in_room {
                        ui.label(
                            egui::RichText::new("В ЭФИРЕ")
                                .size(7.5)
                                .strong()
                                .color(t::LIVE),
                        );
                    }
                    let unread = chat_log.unread(&friend.node_id);
                    if unread > 0 {
                        ui.label(
                            egui::RichText::new(format!("{}", unread))
                                .size(8.5)
                                .strong()
                                .color(Color32::WHITE)
                                .background_color(t::BRAND_2),
                        );
                    }
                });
                ui.horizontal(|ui| {
                    // Точка онлайна: зелёная — узел недавно отвечал на пинг,
                    // серая — офлайн (или о нём ещё ничего не слышно).
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                    ui.painter().circle_filled(
                        dot.center(),
                        2.5,
                        if online { t::LIVE } else { t::TEXT_MUTED },
                    );
                    ui.label(
                        egui::RichText::new(short_node_id(&friend.node_id))
                            .monospace()
                            .size(8.5)
                            .color(t::TEXT_MUTED),
                    );
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let more = t::icon_button(ui, t::Icon::More, 22.0, false).on_hover_text("Ещё");
                let popup_id = ui.make_persistent_id(("friend_menu", &friend.node_id));
                if more.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(popup_id));
                }
                egui::popup_below_widget(
                    ui,
                    popup_id,
                    &more,
                    egui::popup::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(130.0);
                        // «Войти в комнату» — иначе к другу можно попасть
                        // только вставкой его 64-символьного NodeID в рейл.
                        if ui
                            .button(egui::RichText::new("Войти в комнату").size(10.5))
                            .clicked()
                        {
                            let mut target = friend.node_id.clone();
                            crate::widgets::room_panel::join_room(
                                &mut target,
                                p2p.clone(),
                                my_id,
                                is_host.clone(),
                                room.clone(),
                                calls,
                            );
                        }
                        if ui
                            .button(egui::RichText::new("Удалить из друзей").size(10.5))
                            .clicked()
                        {
                            friends.remove_friend(&friend.node_id);
                            if let Some(ref p2p) = p2p {
                                send_remove(p2p, &friend.node_id, my_id, my_node_id);
                            }
                        }
                    },
                );
                if t::icon_button(ui, t::Icon::Chat, 22.0, false)
                    .on_hover_text("Чат")
                    .clicked()
                {
                    chat_log.mark_read(&friend.node_id);
                    *active_chat = Some(friend.node_id.clone());
                    *chat_open = true;
                }
                if t::icon_button(ui, t::Icon::Phone, 22.0, false)
                    .on_hover_text(if !online {
                        "Не в сети"
                    } else if matches!(
                        calls.view(),
                        crate::calls::CallView::Active { .. } | crate::calls::CallView::Outgoing { .. }
                    ) {
                        // Мы уже в комнате — собеседника зовём в неё, а не
                        // открываем второй разговор
                        "Пригласить в комнату"
                    } else {
                        "Позвонить"
                    })
                    .clicked()
                {
                    if online {
                        // Собеседник получает приглашение и сам решает, входить
                        // ли: в комнату он попадает только после «Войти».
                        calls.start_outgoing(&friend.node_id, &friend.name, nick);
                    } else {
                        // Иначе гудок шёл бы 35 секунд в никуда: друг либо
                        // выключил приложение, либо ещё не замечен пингом.
                        *offline_toast = Some(format!("{} не в сети", friend.name));
                    }
                }
            });
        });
    }

    ui.add_space(2.0);
    if !friend_list.is_empty() {
        let (line, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(line, 0.0, t::with_alpha(t::STROKE_SOFT, 0.7));
    }

    // Добавить друга по NodeID
    let mut send = false;
    ui.horizontal(|ui| {
        let field_width = (row_width - 34.0).max(60.0);
        egui::Frame::none()
            .fill(t::INSET)
            .rounding(Rounding::same(8.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(8.0, 5.0))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(add_friend_node)
                        .hint_text("NodeID друга")
                        .desired_width(field_width - 16.0)
                        .frame(false),
                );
            });
        if t::icon_button(ui, t::Icon::Plus, 26.0, false)
            .on_hover_text("Отправить заявку")
            .clicked()
        {
            send = true;
        }
    });

    // Отправка заявки по кнопке «+»
    let target = add_friend_node.trim().to_string();
    let valid = target.len() == 64 && target.chars().all(|c| c.is_ascii_hexdigit());
    if send && valid && !friends.is_friend(&target) {
        if let Some(ref p2p) = p2p {
            send_request(p2p, friends, &target, my_id, my_node_id, nick);
        }
        add_friend_node.clear();
    }
}
