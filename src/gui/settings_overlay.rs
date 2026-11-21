//! Оверлей настроек: звук, экран, рация, система, личность.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Order, Rounding, Stroke};

impl RadioApp {
    pub(super) fn draw_settings_overlay(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut copy = false;

        let scrim = egui::Area::new(egui::Id::new("settings_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let sheet_w = 690.0_f32.min(screen.width() - 60.0);
        let sheet_h = 540.0_f32.min(screen.height() - 70.0);
        let pos = egui::pos2(
            screen.center().x - sheet_w * 0.5,
            screen.center().y - sheet_h * 0.5,
        );

        egui::Area::new(egui::Id::new("settings_sheet"))
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
                        color: egui::Color32::from_black_alpha(170),
                    })
                    .inner_margin(egui::Margin::symmetric(18.0, 14.0))
                    .show(ui, |ui| {
                        ui.set_width(sheet_w - 36.0);
                        ui.set_height(sheet_h - 28.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Настройки")
                                    .size(16.0)
                                    .strong()
                                    .color(t::TEXT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if t::icon_button(ui, t::Icon::Close, 26.0, false)
                                    .on_hover_text("Закрыть")
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });

                        ui.add_space(4.0);
                        t::divider(ui);
                        ui.add_space(6.0);

                        egui::ScrollArea::vertical()
                            .max_height(sheet_h - 90.0)
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                crate::widgets::audio_settings::draw_audio_settings(
                                    ui,
                                    &self.input_devices,
                                    &self.input_channel_counts,
                                    &self.output_devices,
                                    &self.selected_input,
                                    &self.selected_input2,
                                    self.input2_error.as_deref(),
                                    &self.selected_output,
                                    &self.mixer,
                                    &mut self.ptt_bind,
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "ЭКРАН");
                                ui.add_space(4.0);
                                let fps = match self.config.video_fps {
                                    Some(30) => 30,
                                    _ => 60,
                                };
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Кадров в секунду")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    if t::chip_button(ui, "30", fps == 30).clicked() {
                                        self.config.video_fps = Some(30);
                                        self.pending_save = true;
                                    }
                                    if t::chip_button(ui, "60", fps == 60).clicked() {
                                        self.config.video_fps = Some(60);
                                        self.pending_save = true;
                                    }
                                });

                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Буфер показа")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    let mut buffer = self.config.video_buffer_ms.unwrap_or(
                                        crate::network::video_link::DEFAULT_BUFFER_MS,
                                    );
                                    let changed = ui
                                        .add(
                                            egui::Slider::new(&mut buffer, 0..=5000u32)
                                                .step_by(100.0)
                                                .suffix(" мс"),
                                        )
                                        .changed();
                                    if changed {
                                        self.config.video_buffer_ms = Some(buffer);
                                        self.pending_save = true;
                                        if let Some(link) = &self.video_link {
                                            link.set_buffer_ms(buffer);
                                        }
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(
                                        "Буфер зрителя: сеть успевает отдать отставший кадр. 0 — показывать сразу.",
                                    )
                                    .size(8.5)
                                    .color(t::TEXT_MUTED),
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "СИСТЕМА");
                                ui.add_space(4.0);
                                let mut close_to_tray =
                                    self.config.close_to_tray.unwrap_or(true);
                                if ui
                                    .checkbox(
                                        &mut close_to_tray,
                                        egui::RichText::new(
                                            "Закрывать в трей, не выходя из эфира",
                                        )
                                        .size(10.5)
                                        .color(t::TEXT_MUTED),
                                    )
                                    .changed()
                                {
                                    self.config.close_to_tray = Some(close_to_tray);
                                    self.pending_save = true;
                                }
                                ui.add_space(4.0);
                                let mut autostart = self.autostart_on;
                                if ui
                                    .checkbox(
                                        &mut autostart,
                                        egui::RichText::new("Запускать при входе в Windows")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    )
                                    .changed()
                                {
                                    if crate::autostart::set_enabled(autostart) {
                                        self.autostart_on = autostart;
                                        self.show_toast(if autostart {
                                            "Автозапуск включён"
                                        } else {
                                            "Автозапуск выключен"
                                        });
                                    } else {
                                        self.show_toast("Не удалось изменить автозапуск");
                                    }
                                }
                                ui.label(
                                    egui::RichText::new(
                                        "Трей: двойной клик по иконке открывает окно, меню — выход.",
                                    )
                                    .size(8.5)
                                    .color(t::TEXT_MUTED),
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "ЛИЧНОСТЬ");
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Ваше имя")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.nick)
                                            .desired_width(180.0),
                                    );
                                });

                                ui.add_space(6.0);
                                t::inset_frame().show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&self.my_node_id)
                                                .monospace()
                                                .size(9.0)
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if t::chip_button(ui, "Копировать", false)
                                                    .clicked()
                                                {
                                                    copy = true;
                                                }
                                            },
                                        );
                                    });
                                });

                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Сбросить личность")
                                                .size(10.5)
                                                .strong()
                                                .color(t::REC),
                                        )
                                        .rounding(Rounding::same(8.0)),
                                    )
                                    .clicked()
                                {
                                    crate::identity::Identity::reset();
                                    self.reset_note = Some(
                                        "Новый NodeID создан. Перезапустите приложение."
                                            .to_string(),
                                    );
                                }
                                if let Some(note) = &self.reset_note {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(note)
                                            .size(9.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                }
                            });
                    });
            });

        if copy {
            self.copy_node_id(ctx);
        }
        if close {
            self.settings_open = false;
        }
    }
}
