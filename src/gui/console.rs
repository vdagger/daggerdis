//! Консоль эфира: диск, карточки, PTT-пилюля, рейл и тосты.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Order, Rounding, Stroke};
use std::time::Duration;

impl RadioApp {
    pub(super) fn draw_console(&mut self, ui: &mut egui::Ui, level: f32, transmitting: bool, wave: &[f32]) {
        let peers = self.room.get_all_peers();
        let is_host = self.is_host.load(std::sync::atomic::Ordering::Relaxed);

        let total_h = (ui.available_height() - 2.0).max(420.0);
        let mut open_chat = false;
        let mut open_diagnostics = false;

        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(14.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(total_h - 24.0);

                // Заголовок эфира — внутри панели, как в макете
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, t::LIVE);
                    ui.label(
                        egui::RichText::new("Эфир")
                            .size(14.0)
                            .strong()
                            .color(t::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(members_label(peers.len() + 1))
                            .size(9.5)
                            .color(t::TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if is_host {
                            t::badge(ui, "ХОСТ", t::GOLD);
                        }
                        // Чат — в шапке эфира: в подвале бейдж непрочитанного
                        // обрезался за нижним краем окна при размере по умолчанию.
                        ui.add_space(8.0);
                        let unread = self.unread_total();
                        if chat_chip(ui, unread, self.chat_open).clicked() {
                            open_chat = true;
                        }
                        ui.add_space(6.0);
                        // Диагностика рядом с чатом: когда «плохо слышно», её
                        // ищут в первую очередь, а не в настройках.
                        if t::chip_button(ui, "Диагностика", self.diag_open).clicked() {
                            open_diagnostics = true;
                        }
                    });
                });

                // Геометрия снизу вверх, как в макете: футер прижат к низу
                // карточки, над ним PTT, полоса линии и центральный блок.
                let wave_h = 56.0;
                let ptt_h = 54.0;
                let footer_h = 26.0;

                let full = ui.max_rect();
                let bottom = full.bottom();
                let footer_top = bottom - footer_h;
                let ptt_top = footer_top - 24.0 - ptt_h;
                let wave_top = ptt_top - 8.0 - wave_h;
                let mid_top = ui.cursor().top() + 4.0;
                let mid_bottom = (wave_top - 8.0).max(mid_top + 200.0);
                let mid_h = mid_bottom - mid_top;

                let dial_w = (full.width() * 0.46).clamp(230.0, 330.0);

                let mid_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), mid_top),
                    egui::pos2(full.right(), mid_bottom),
                );
                ui.allocate_ui_at_rect(mid_rect, |ui| {
                    ui.horizontal_top(|ui| {
                        // Герой в макете сидит ближе к низу блока
                        let pad = (mid_h - 315.0 - 12.0).max(0.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(dial_w, mid_h),
                            Layout::top_down(Align::Center),
                            |ui| {
                                ui.add_space(pad);
                                crate::widgets::hero::draw_hero(
                                    ui,
                                    level,
                                    transmitting,
                                    is_host,
                                    wave,
                                    peers.len(),
                                    &self.nick,
                                );
                            },
                        );

                        ui.add_space(4.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(full.width() - dial_w - 20.0, mid_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.add_space(pad);
                                self.draw_stream_card(ui);
                                ui.add_space(8.0);
                                self.draw_mixer_card(ui);
                            },
                        );
                    });
                });

                let wave_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), wave_top),
                    egui::pos2(full.right(), wave_top + wave_h),
                );
                ui.allocate_ui_at_rect(wave_rect, |ui| {
                    crate::widgets::hero::draw_wave_strip(ui, wave, transmitting, wave_h);
                });

                let ptt_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), ptt_top),
                    egui::pos2(full.right(), ptt_top + ptt_h),
                );
                ui.allocate_ui_at_rect(ptt_rect, |ui| {
                    crate::widgets::ptt_button::draw_ptt_button(
                        ui,
                        &self.ptt_flag,
                        &mut self.continuous_mode,
                        &self.mixer,
                        self.ptt_bind,
                    );
                });

                let footer_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), footer_top),
                    egui::pos2(full.right(), bottom),
                );
                ui.allocate_ui_at_rect(footer_rect, |ui| {
                    ui.horizontal(|ui| {
                        crate::widgets::ptt_button::draw_continuous_toggle(
                            ui,
                            &self.ptt_flag,
                            &mut self.continuous_mode,
                        );
                        ui.add_space(10.0);
                        crate::widgets::ptt_button::draw_mic_mute_toggle(ui, &self.mixer);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} FPS",
                                    self.config.video_fps.unwrap_or(60)
                                ))
                                .size(9.0)
                                .monospace()
                                .color(t::TEXT_MUTED),
                            );
                        });
                    });
                });
            });

        if open_chat {
            self.open_chat();
        }
        if open_diagnostics {
            self.diag_open = !self.diag_open;
        }
    }

    pub(super) fn draw_rail(&mut self, ui: &mut egui::Ui) {
        // Как в макете: одна карточка на весь рейл, секции — волосяными
        // линиями во всю ширину, подвал прижат к низу внутри карточки.
        let footer_h = 28.0;
        let mut do_leave = false;

        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(t::RADIUS))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let scroll_h = (ui.available_height() - footer_h).max(240.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(scroll_h)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        let mut copy = false;
                        rail_section(ui, |ui| {
                            if crate::widgets::room_panel::draw_room_connect(
                                ui,
                                &mut self.connect_ip_input,
                                self.iroh.clone(),
                                self.my_id,
                                self.is_host.clone(),
                                self.room.clone(),
                                &self.calls,
                            ) {
                                do_leave = true;
                            }
                        });
                        rail_divider(ui);

                        rail_section(ui, |ui| {
                            copy = crate::widgets::room_panel::draw_my_nodeid(
                                ui,
                                &self.my_node_id,
                            );
                        });
                        if copy {
                            self.copy_node_id(ui.ctx());
                        }
                        rail_divider(ui);

                        rail_section(ui, |ui| {
                            crate::widgets::room_panel::draw_participants(
                                ui,
                                self.room.clone(),
                                &self.mixer,
                                self.is_host.clone(),
                                self.ptt_flag.load(std::sync::atomic::Ordering::Relaxed),
                                &self.nick,
                                &self.friends,
                            );
                        });
                        rail_divider(ui);

                        let mut offline_toast: Option<String> = None;
                        rail_section(ui, |ui| {
                            crate::widgets::friends_panel::draw_friends_panel(
                                ui,
                                &self.friends,
                                self.iroh.clone(),
                                self.my_id,
                                &self.my_node_id,
                                &self.nick,
                                &mut self.add_friend_node,
                                &self.chat_log,
                                &mut self.active_chat,
                                &mut self.chat_open,
                                &self.calls,
                                &self.presence,
                                &mut offline_toast,
                                &self.room,
                                &self.is_host,
                            );
                        });
                        if let Some(text) = offline_toast {
                            self.show_toast(&text);
                        }
                        if self.chat_open {
                            self.settings_open = false;
                        }
                    });

                // Подвал внутри карточки
                rail_divider(ui);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 2.5, t::LIVE);
                    ui.add_space(6.0);
                    t::micro(ui, "Сквозное шифрование");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(14.0);
                        t::micro(ui, "v0.1");
                    });
                });
            });

        if do_leave {
            self.leave_voice();
        }
    }

    pub(super) fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_millis(2400) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(60));

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("node_toast"))
            .fixed_pos(egui::pos2(screen.center().x, screen.bottom() - 92.0))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE_HI)
                    .rounding(Rounding::same(9.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 10.0),
                        blur: 26.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(150),
                    })
                    .inner_margin(egui::Margin::symmetric(12.0, 7.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 3.0, t::LIVE);
                            ui.label(egui::RichText::new(text).size(10.5).color(t::TEXT));
                        });
                    });
            });
    }
}
