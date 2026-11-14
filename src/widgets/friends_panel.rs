//! Список друзей, входящие заявки и добавление нового друга

use crate::friends::FriendsManager;
use crate::gui::theme as t;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::network::NetworkManager;
use eframe::egui;
use egui::{Color32, Rounding, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn short_node_id(node_id: &str) -> String {
    if node_id.len() > 16 {
        format!("{}…", &node_id[..15])
    } else {
        node_id.to_string()
    }
}

fn dial(p2p: &Arc<IrohP2pManager>, target: &str, my_id: u64) {
    let p2p = p2p.clone();
    let target = target.to_string();
    tokio::task::spawn(async move {
        if let Err(e) = p2p.connect_to_peer(&target).await {
            eprintln!("[ДРУЗЬЯ] Не удалось дозвониться до {}: {}", target, e);
            return;
        }
        let hello = crate::network::NetworkMessage::Heartbeat {
            sender_id: my_id,
            is_host: false,
        };
        let bytes = crate::network::message::serialize_msg(&hello);
        p2p.broadcast_bytes(&bytes).await;
    });
}

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
        if p2p.connect_to_peer(&target).await.is_err() {
            return;
        }
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
        let _ = p2p.connect_to_peer(&target).await;
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

fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(t::SURFACE)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0_f32, t::STROKE))
        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
}

pub fn draw_friends_panel(
    ui: &mut egui::Ui,
    friends: &Arc<FriendsManager>,
    network: Arc<NetworkManager>,
    my_id: u64,
    my_node_id: &str,
    nick: &str,
    is_host: Arc<AtomicBool>,
    add_friend_node: &mut String,
) {
    let row_width = ui.available_width();
    let p2p = network.iroh.clone();

    ui.horizontal(|ui| {
        t::section_label(ui, "ДРУЗЬЯ");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{}", friends.friends().len()))
                    .size(10.0)
                    .color(t::TEXT_MUTED),
            );
        });
    });

    ui.add_space(4.0);

    // Входящие заявки
    let incoming = friends.incoming();
    for req in incoming {
        card_frame()
            .show(ui, |ui| {
                ui.set_width(row_width - 20.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&req.nick)
                                .size(12.5)
                                .strong()
                                .color(t::TEXT),
                        );
                        ui.label(
                            egui::RichText::new(format!("Заявка · {}", short_node_id(&req.node_id)))
                                .size(9.5)
                                .color(t::TEXT_MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Отклонить")
                                        .size(10.5)
                                        .color(t::TEXT_DIM),
                                )
                                .rounding(Rounding::same(7.0)),
                            )
                            .clicked()
                        {
                            friends.decline_incoming(&req.node_id);
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Принять")
                                        .size(10.5)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(t::BRAND_DEEP)
                                .rounding(Rounding::same(7.0)),
                            )
                            .clicked()
                        {
                            let accepted = friends.accept_incoming(&req.node_id);
                            if accepted.is_some() {
                                if let Some(ref p2p) = p2p {
                                    is_host.store(false, Ordering::SeqCst);
                                    send_accept(p2p, &req.node_id, my_id, my_node_id, nick);
                                }
                            }
                        }
                    });
                });
            });
        ui.add_space(3.0);
    }

    // Список друзей
    for friend in friends.friends() {
        card_frame()
            .show(ui, |ui| {
                ui.set_width(row_width - 20.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&friend.name)
                                .size(12.5)
                                .strong()
                                .color(t::TEXT),
                        );
                        ui.label(
                            egui::RichText::new(short_node_id(&friend.node_id))
                                .monospace()
                                .size(9.5)
                                .color(t::TEXT_MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Удалить")
                                        .size(10.5)
                                        .color(t::TEXT_DIM),
                                )
                                .rounding(Rounding::same(7.0)),
                            )
                            .clicked()
                        {
                            friends.remove_friend(&friend.node_id);
                            if let Some(ref p2p) = p2p {
                                send_remove(p2p, &friend.node_id, my_id, my_node_id);
                            }
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Позвонить")
                                        .size(10.5)
                                        .strong()
                                        .color(Color32::WHITE),
                                )
                                .fill(t::BRAND_DEEP)
                                .rounding(Rounding::same(7.0)),
                            )
                            .clicked()
                        {
                            if let Some(ref p2p) = p2p {
                                is_host.store(false, Ordering::SeqCst);
                                dial(p2p, &friend.node_id, my_id);
                            }
                        }
                    });
                });
            });
        ui.add_space(3.0);
    }

    ui.add_space(6.0);

    // Добавить друга по NodeID
    ui.add(
        egui::TextEdit::singleline(add_friend_node)
            .hint_text("NodeID друга")
            .desired_width(row_width),
    );

    ui.add_space(4.0);
    if ui
        .add_sized(
            [row_width, 24.0],
            egui::Button::new(
                egui::RichText::new("Отправить заявку")
                    .size(11.5)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(t::BRAND_DEEP)
            .rounding(Rounding::same(8.0)),
        )
        .clicked()
    {
        let target = add_friend_node.trim().to_string();
        if target.len() == 64
            && target.chars().all(|c| c.is_ascii_hexdigit())
            && !friends.is_friend(&target)
        {
            if let Some(ref p2p) = p2p {
                is_host.store(false, Ordering::SeqCst);
                send_request(p2p, friends, &target, my_id, my_node_id, nick);
            }
            add_friend_node.clear();
        }
    }
}
