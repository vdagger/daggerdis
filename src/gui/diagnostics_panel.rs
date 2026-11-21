//! Панель «Диагностика»: числа сети, звука и видео и отчёт в буфер.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Order, Rounding, Stroke};
use std::time::{Duration, Instant};

impl RadioApp {
    /// Панель «Диагностика»: сеть, звук, видео и кнопка «Скопировать отчёт».
    ///
    /// Числа берутся из того же `ReportInput`, что уходит в буфер обмена, —
    /// экран и отчёт не расходятся. Панель не управляет звуком и видео: это
    /// только наблюдение, поэтому и прав тут нет.
    pub(super) fn draw_diagnostics(&mut self, ctx: &egui::Context) {
        let input = self.diagnostics_input();
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut copy = false;

        let scrim = egui::Area::new(egui::Id::new("diag_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let sheet_w = 620.0_f32.min(screen.width() - 60.0);
        let sheet_h = 560.0_f32.min(screen.height() - 70.0);
        let pos = egui::pos2(
            screen.center().x - sheet_w * 0.5,
            screen.center().y - sheet_h * 0.5,
        );

        egui::Area::new(egui::Id::new("diag_sheet"))
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
                                egui::RichText::new("Диагностика")
                                    .size(16.0)
                                    .strong()
                                    .color(t::TEXT),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("наблюдение, ничего не меняет")
                                    .size(9.5)
                                    .color(t::TEXT_MUTED),
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
                            .max_height(sheet_h - 132.0)
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                draw_diagnostics_body(ui, &input);
                            });

                        ui.add_space(6.0);
                        t::divider(ui);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if t::primary_button(
                                ui,
                                "Скопировать отчёт",
                                egui::vec2(180.0, 30.0),
                            )
                            .clicked()
                            {
                                copy = true;
                            }
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "отчёт можно вставить в чат: у собеседника будут те же числа",
                                )
                                .size(9.5)
                                .color(t::TEXT_MUTED),
                            );
                        });
                    });
            });

        if copy {
            self.copy_diagnostics(ctx);
        }
        // Esc закрывает панель, как и клик по затемнению
        if close || (!ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)))
        {
            self.diag_open = false;
        }
    }

    /// Всё, что попадает в отчёт и в панель: собирается в одном месте, чтобы
    /// экран и скопированный текст показывали одни и те же числа.
    pub(super) fn diagnostics_input(&self) -> crate::diagnostics::ReportInput {
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        let friends = self.friends.friends();
        let friends_online = friends
            .iter()
            .filter(|f| self.presence.is_online(&f.node_id))
            .count();
        let links = self
            .iroh
            .as_ref()
            .map(|p2p| p2p.link_stats())
            .unwrap_or_default();

        let mut errors = Vec::new();
        if let Some(e) = &self.network_error {
            errors.push(e.clone());
        }
        if let Some(e) = self.input2_error.as_deref() {
            errors.push(format!("второй вход: {e}"));
        }

        let mut own_video: Option<crate::diagnostics::OwnVideo> = None;
        let mut remote_video: Option<crate::diagnostics::RemoteVideo> = None;
        #[cfg(windows)]
        {
            if let Some(video) = &self.video {
                let (decode_ms, gpu_ms, image_ms) = video.timings();
                own_video = Some(crate::diagnostics::OwnVideo {
                    show_fps: video.fps(),
                    target_fps: video.info().fps,
                    encode_fps: video.encode_fps(),
                    kbps: video.encode_kbps(),
                    decode_ms,
                    gpu_ms,
                    image_ms,
                });
                if let Some(e) = video.last_error() {
                    errors.push(format!("видео: {e}"));
                }
            }
            if let Some(remote) = &self.remote {
                let (decode_ms, gpu_ms, image_ms) = remote.timings();
                remote_video = Some(crate::diagnostics::RemoteVideo {
                    peer: remote.peer.clone(),
                    fps: remote.fps(),
                    decode_ms,
                    gpu_ms,
                    image_ms,
                    buffer_ms: self.config.video_buffer_ms.unwrap_or(2000),
                    frames: self.remote_frames,
                });
                if let Some(e) = remote.last_error() {
                    errors.push(format!("приём видео: {e}"));
                }
            }
            if let Some(e) = &self.video_error {
                errors.push(format!("видео: {e}"));
            }
        }

        let input2 = self.selected_input2.load(relaxed) as usize;

        crate::diagnostics::ReportInput {
            node_id: self.my_node_id.clone(),
            is_host: self.is_host.load(relaxed),
            // +1 — сам узел: в комнате он тоже участник, и шапка «1 узел»
            // считает так же. Иначе панель и шапка показывали разные числа.
            members_in_room: self.room.get_all_peers().len() + 1,
            left_room: self.room.is_left(),
            friends_online,
            friends_total: friends.len(),
            links,
            rx_total: self.diagnostics.rx.snapshot(),
            rx_window: self.diag_window,
            rx_window_secs: self.diag_window_secs.max(1.0),
            mic_device: self
                .input_devices
                .get(self.selected_input.load(relaxed) as usize)
                .cloned(),
            input2_device: if input2 == 0 {
                None
            } else {
                self.input_devices.get(input2 - 1).cloned()
            },
            output_device: self
                .output_devices
                .get(self.selected_output.load(relaxed) as usize)
                .cloned(),
            mic_muted: self.mixer.is_mic_muted(),
            mic_level: self.mixer.input1_peak().max(self.mixer.input2_peak()),
            opus_kbps: self.mixer.opus_bitrate() / 1000,
            own_video,
            remote_video,
            errors,
            uptime: self.diagnostics.uptime(),
        }
    }

    /// Раз в секунду пересчитывает окно счётчиков приёма.
    ///
    /// Накопительные числа отвечают на вопрос «что было за сессию», а окно —
    /// «что происходит прямо сейчас»: при разборе «робота» важно второе.
    pub(super) fn sample_diagnostics(&mut self) {
        let elapsed = self.diag_prev.1.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let current = self.diagnostics.rx.snapshot();
        self.diag_window = self.diag_prev.0.delta(&current);
        self.diag_window_secs = elapsed.as_secs_f32();
        self.diag_prev = (current, Instant::now());
        self.tune_voice();
    }

    /// «Скопировать отчёт»: тот же текст, что видно в панели, но целиком и
    /// с полным NodeID — его отправляют собеседнику при разборе «плохо слышно».
    pub(super) fn copy_diagnostics(&mut self, ctx: &egui::Context) {
        let report = crate::diagnostics::render(&self.diagnostics_input());
        // Заодно в журнал: консоль под рукой у того, кто смотрит процесс.
        println!("{report}");
        ctx.output_mut(|o| o.copied_text = report);
        self.show_toast("Отчёт диагностики скопирован");
    }
}
