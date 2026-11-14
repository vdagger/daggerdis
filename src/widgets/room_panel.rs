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

fn short_node_id(node_id: &str) -> String {
    if node_id.len() > 16 {
        format!("{}…", &node_id[..15])
    } else {
        node_id.to_string()
    }
}

/// Компактный аватар с кольцом состояния
fn avatar(ui: &mut egui::Ui, speaking: bool, level: f32, time: f64) {
    let size = 32.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter_at(rect);
    let center = rect.center();
    let radius = 15.0;

    let ring = if speaking {
        1.0 + 0.05 * (time * 6.0).sin() as f32
    } else {
        1.0
    };

    painter.circle_filled(center, 13.0, t::SURFACE_HI);
    painter.circle_stroke(
        center,
        radius * ring,
        Stroke::new(
            if speaking { 1.8_f32 } else { 1.0_f32 },
            if speaking { t::LIVE } else { t::STROKE },
        ),
    );

    let dot = 3.0 + 7.0 * level.clamp(0.0, 1.0);
    painter.circle_filled(
        center,
        dot,
        if speaking { t::LIVE } else { t::TEXT_MUTED },
    );
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

pub fn draw_participants(
    ui: &mut egui::Ui,
    room: std::sync::Arc<crate::room::RoomManager>,
    mixer: &crate::mixer::AudioMixer,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    is_transmitting: bool,
) {
    let time = ui.input(|i| i.time);
    let now = now_ms();
    let peers = room.get_all_peers();
    let my_is_host = is_host.load(Ordering::Relaxed);

    ui.horizontal(|ui| {
        t::section_label(ui, "УЧАСТНИКИ");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{}", peers.len() + 1))
                    .size(10.0)
                    .color(t::TEXT_MUTED),
            );
        });
    });

    ui.add_space(4.0);

    let row_width = ui.available_width();

    // Себя показываем строкой без регулятора громкости
    egui::Frame::none()
        .fill(t::SURFACE)
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
        .show(ui, |ui| {
            ui.set_width(row_width - 20.0);
            ui.horizontal(|ui| {
                avatar(ui, is_transmitting, if is_transmitting { 0.5 } else { 0.0 }, time);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Вы")
                            .size(12.5)
                            .strong()
                            .color(t::TEXT),
                    );
                    let (status, color) = status_text(is_transmitting, true);
                    ui.label(egui::RichText::new(status).size(10.0).color(color));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if my_is_host {
                        t::badge(ui, "ХОСТ", t::GOLD);
                    }
                    ui.label(
                        egui::RichText::new(format!("{}", my_id))
                            .monospace()
                            .size(9.5)
                            .color(t::TEXT_MUTED),
                    );
                });
            });
        });

    ui.add_space(3.0);

    for peer in peers {
        let alive = now.saturating_sub(peer.last_heartbeat) < 3000;
        let speaking = mixer.is_peer_speaking(peer.id);
        let level = mixer.peer_meter(peer.id).peak;

        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
            .show(ui, |ui| {
                ui.set_width(row_width - 20.0);
                ui.horizontal(|ui| {
                    avatar(ui, speaking, level, time);
                    ui.add_space(2.0);

                    ui.vertical(|ui| {
                        let id_str = peer.id.to_string();
                        let short_id = &id_str[..4.min(id_str.len())];
                        ui.label(
                            egui::RichText::new(format!("Гость {}", short_id))
                                .size(12.5)
                                .strong()
                                .color(t::TEXT),
                        );
                        let (status, color) = status_text(speaking, alive);
                        ui.label(egui::RichText::new(status).size(10.0).color(color));
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut volume = mixer.get_peer_volume(peer.id);
                        ui.spacing_mut().slider_width = 64.0;
                        if ui
                            .add(
                                egui::Slider::new(&mut volume, 0..=300)
                                    .show_value(false),
                            )
                            .changed()
                        {
                            mixer.set_peer_volume(peer.id, volume);
                        }
                        ui.label(
                            egui::RichText::new(format!("{}%", volume))
                                .size(9.5)
                                .color(t::TEXT_MUTED),
                        );

                        if peer.is_host {
                            t::badge(ui, "ХОСТ", t::GOLD);
                        }
                    });
                });
            });

        ui.add_space(3.0);
    }
}

pub fn draw_room_panel(
    ui: &mut egui::Ui,
    connect_ip: &mut String,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    room: std::sync::Arc<crate::room::RoomManager>,
) {
    t::section_label(ui, "КОМНАТА");

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let btn_width = 74.0;
        let spacing = ui.spacing().item_spacing.x;
        let input_width = (ui.available_width() - btn_width - spacing).max(80.0);

        ui.add(
            egui::TextEdit::singleline(connect_ip)
                .hint_text("NodeID собеседника")
                .desired_width(input_width),
        );

        let btn = egui::Button::new(
            egui::RichText::new("Войти")
                .size(12.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(t::BRAND_DEEP)
        .rounding(Rounding::same(8.0));

        if ui.add_sized([btn_width, 26.0], btn).clicked() {
            join_room(connect_ip, iroh.clone(), my_id, is_host.clone());
        }
    });

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
                .size(10.0)
                .color(t::TEXT_MUTED),
        );
    }
}

fn join_room(
    connect_ip: &mut String,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
