//! Выдвижной чат: вкладки «Личка» и «Комната», непрочитанное.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Order, Rounding, Stroke};

impl RadioApp {
    pub(super) fn draw_chat_overlay(&mut self, ctx: &egui::Context) {
        if !self.chat_open {
            return;
        }
        let room_alive = !self.room.get_all_peers().is_empty() && !self.room.is_left();
        // Вкладка «Комната» при пустом эфире бессмысленна: назад в личку.
        if self.chat_room && !room_alive && self.active_chat.is_some() {
            self.chat_room = false;
        }
        let node_id = if self.chat_room {
            String::new()
        } else {
            match self.active_chat.clone() {
                Some(id) => id,
                None => {
                    self.chat_open = false;
                    return;
                }
            }
        };
        let show_room = self.chat_room;
        let room_unread = self.room_chat.unread();
        let room_id = self.calls.room_id().unwrap_or(0);
        let mut want_room: Option<bool> = None;
        let screen = ctx.screen_rect();
        let mut close = false;

        let scrim = egui::Area::new(egui::Id::new("chat_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(80));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let width = 392.0_f32.min(screen.width() - 20.0);
        let height = screen.height() - 20.0;
        let pos = egui::pos2(screen.right() - width - 10.0, screen.top() + 10.0);

        egui::Area::new(egui::Id::new("chat_drawer"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(16.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 18.0),
                        blur: 50.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(160),
                    })
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 28.0);
                        ui.set_height(height - 24.0);
                        // Вкладки: личка другу и общий чат комнаты. Без них
                        // «Чат» открывал только переписку, и написать в эфир
                        // было нечем.
                        ui.horizontal(|ui| {
                            if t::chip_button(ui, "Личка", !show_room).clicked() {
                                want_room = Some(false);
                            }
                            let label = if room_unread > 0 {
                                format!("Комната · {room_unread}")
                            } else {
                                "Комната".to_string()
                            };
                            let resp = t::chip_button(ui, &label, show_room);
                            if resp.clicked() && room_alive {
                                want_room = Some(true);
                            }
                            if !room_alive {
                                resp.on_hover_text("В эфире никого нет");
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if t::icon_button(ui, t::Icon::Close, 24.0, false)
                                    .on_hover_text("Закрыть")
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(4.0);
                        t::divider(ui);
                        ui.add_space(3.0);
                        if show_room {
                            crate::widgets::chat_panel::draw_room_chat_panel(
                                ui,
                                &self.room_chat,
                                &self.nick,
                                self.my_id,
                                &self.room,
                                room_id,
                                self.iroh.clone(),
                                &mut self.chat_input,
                                &mut close,
                                &self.chat_error,
                            );
                        } else {
                            crate::widgets::chat_panel::draw_chat_panel(
                                ui,
                                &self.chat_log,
                                &self.friends,
                                &node_id,
                                &mut self.chat_input,
                                self.iroh.clone(),
                                self.my_id,
                                &mut close,
                                &self.chat_error,
                                &self.presence,
                            );
                        }
                    });
            });

        if let Some(room) = want_room {
            if self.chat_room != room {
                self.chat_input.clear();
            }
            self.chat_room = room;
        }

        // Открытый чат считается прочитанным: раньше счётчик копился вечно,
        // и бейдж горел даже при уже просмотренной переписке.
        if self.chat_room {
            self.room_chat.mark_read();
        } else {
            self.chat_log.mark_read(&node_id);
            self.seen_unread.insert(node_id.clone(), 0);
        }

        if close {
            self.chat_open = false;
            self.chat_input.clear();
        }
    }

    pub(super) fn open_chat(&mut self) {
        if self.active_chat.is_none() {
            self.active_chat = self.friends.friends().first().map(|f| f.node_id.clone());
        }
        // Друзей может не быть вовсе, а эфир — быть: тогда чат открывается
        // сразу на комнате, иначе кнопка «Чат» выглядела бы сломанной.
        if self.active_chat.is_none() && !self.room.get_all_peers().is_empty() {
            self.chat_room = true;
        }
        self.chat_open = self.active_chat.is_some() || self.chat_room;
        self.settings_open = false;
    }

    pub(super) fn unread_total(&self) -> usize {
        let dm: usize = self
            .friends
            .friends()
            .iter()
            .map(|f| self.chat_log.unread(&f.node_id))
            .sum();
        dm + self.room_chat.unread()
    }
}
