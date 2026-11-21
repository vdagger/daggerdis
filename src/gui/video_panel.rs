//! Окно показа: своя трансляция, чужие потоки, карточки трансляции и микшера.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Rounding, Stroke};
use std::time::{Duration, Instant};

impl RadioApp {
    /// Окно просмотра: отдельное, растягиваемое, с полноэкранным режимом.
    #[cfg(windows)]
    pub(super) fn draw_video_window(&mut self, ctx: &egui::Context) {
        let has_source = self.video.is_some() || self.remote.is_some();
        if !self.video_window || !has_source {
            self.video_window = false;
            self.video_fullscreen = false;
            return;
        }

        let close_requested = std::cell::Cell::new(false);
        let chrome = std::cell::Cell::new(Instant::now());
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Albus — трансляция")
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([360.0, 240.0])
            .with_resizable(true);
        if self.video_fullscreen {
            builder = builder.with_fullscreen(true);
        }

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("video-window"),
            builder,
            |ctx, _class| {
                let pointer_active = ctx.input(|i| i.pointer.is_moving());
                if pointer_active {
                    chrome.set(Instant::now());
                }
                let show_bar = chrome.get().elapsed() < Duration::from_secs(3);

                if show_bar {
                    egui::TopBottomPanel::top("video-window-bar")
                        .frame(
                            egui::Frame::none()
                                .fill(egui::Color32::from_black_alpha(190))
                                .inner_margin(egui::Margin::symmetric(12.0, 7.0)),
                        )
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                let label = if self.video_fullscreen {
                                    "Выйти из полноэкранного"
                                } else {
                                    "На весь экран"
                                };
                                if t::chip_button(ui, label, false).clicked() {
                                    self.video_fullscreen = !self.video_fullscreen;
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                        self.video_fullscreen,
                                    ));
                                }
                                if let Some(remote) = &self.remote {
                                    // миллисекунды по этапам: по ним зритель видит,
                                    // упирается показ в его же компьютер или в сеть
                                    let (decode, gpu, image) = remote.timings();
                                    let buffer = self
                                        .config
                                        .video_buffer_ms
                                        .unwrap_or(crate::network::video_link::DEFAULT_BUFFER_MS)
                                        as f32
                                        / 1000.0;
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Смотрит: {} · {:.0} FPS · буфер {:.1} с · декод {:.1} / GPU {:.1} / картинка {:.1} мс",
                                            remote.peer,
                                            remote.fps(),
                                            buffer,
                                            decode,
                                            gpu,
                                            image
                                        ))
                                        .size(9.5)
                                        .color(t::TEXT_MUTED),
                                    );
                                } else if let Some(video) = &self.video {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {:.0} FPS",
                                            video.source,
                                            video.fps()
                                        ))
                                        .size(9.5)
                                        .color(t::TEXT_MUTED),
                                    );
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if t::icon_button(ui, t::Icon::Close, 24.0, false)
                                        .on_hover_text("Закрыть")
                                        .clicked()
                                    {
                                        close_requested.set(true);
                                    }
                                    ui.label(
                                        egui::RichText::new("Esc — выйти")
                                            .size(9.0)
                                            .color(t::TEXT_MUTED),
                                    );
                                });
                            });
                        });
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        // приоритет у чужого потока
                        let source = self
                            .remote
                            .as_ref()
                            .map(|r| (r.texture(), r.size()))
                            .or_else(|| self.video.as_ref().map(|v| (v.texture(), v.size())));
                        if let Some((Some(tex), (w, h))) = source {
                            let rect = ui.max_rect();
                            let scale =
                                (rect.width() / w as f32).min(rect.height() / h as f32);
                            let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                            let img = egui::Image::new(egui::load::SizedTexture::new(
                                tex.id(),
                                size,
                            ))
                            .sense(egui::Sense::click());
                            let resp = ui.centered_and_justified(|ui| ui.add(img)).inner;
                            if resp.double_clicked() {
                                self.video_fullscreen = !self.video_fullscreen;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                    self.video_fullscreen,
                                ));
                            }
                        } else {
                            // Чёрное окно без объяснения — худший вариант: ошибку
                            // декодера/подписки показываем прямо здесь.
                            let error = self
                                .remote
                                .as_ref()
                                .and_then(|r| r.last_error().cloned())
                                .or_else(|| {
                                    self.video.as_ref().and_then(|v| v.last_error().cloned())
                                })
                                .or_else(|| self.video_error.clone());
                            let text = error
                                .map(|e| format!("Видео недоступно: {e}"))
                                .unwrap_or_else(|| "Ожидание кадра…".to_string());
                            ui.centered_and_justified(|ui| {
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(11.0)
                                        .color(t::TEXT_MUTED),
                                );
                            });
                        }
                    });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if self.video_fullscreen {
                        self.video_fullscreen = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    } else {
                        close_requested.set(true);
                    }
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested.set(true);
                }

                if show_bar {
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            },
        );

        if close_requested.get() {
            self.video_window = false;
            self.video_fullscreen = false;
        }
    }

    /// Карточка трансляции: миниатюра слева, данные и управление справа
    #[cfg(windows)]
    pub(super) fn draw_stream_card(&mut self, ui: &mut egui::Ui) {
        let running = self.video.is_some();
        let mut open_window = false;

        egui::Frame::none()
            .fill(t::INNER)
            .rounding(Rounding::same(10.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(9.0, 9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal_top(|ui| {
                    let size = egui::vec2(128.0, 72.0);
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let resp = resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Открыть окно трансляции");
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, Rounding::same(6.0), t::INSET);
                    painter.rect_stroke(
                        rect,
                        Rounding::same(6.0),
                        Stroke::new(1.0_f32, t::STROKE_SOFT),
                    );

                    let preview = self
                        .remote
                        .as_ref()
                        .map(|r| (r.texture(), r.size()))
                        .or_else(|| self.video.as_ref().map(|v| (v.texture(), v.size())));
                    let mut has_frame = false;
                    if let Some((Some(tex), (w, h))) = preview {
                        // вписываем кадр в миниатюру, сохраняя пропорции
                        let src_ratio = w.max(1) as f32 / h.max(1) as f32;
                        let box_ratio = rect.width() / rect.height();
                        let target = if src_ratio > box_ratio {
                            egui::vec2(rect.width(), rect.width() / src_ratio)
                        } else {
                            egui::vec2(rect.height() * src_ratio, rect.height())
                        };
                        let frame = egui::Rect::from_center_size(rect.center(), target);
                        painter.image(
                            tex.id(),
                            frame,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );
                        has_frame = true;
                    }
                    if !has_frame {
                        // Заглушка окна: без активного потока превью пустое
                        let inner = rect.shrink(8.0);
                        painter.rect_filled(
                            inner,
                            Rounding::same(4.0),
                            egui::Color32::from_rgb(19, 19, 23),
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                inner.min,
                                egui::vec2(inner.width(), 6.0),
                            ),
                            Rounding::same(4.0),
                            egui::Color32::from_rgb(31, 31, 37),
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(inner.right() - 26.0, inner.top() + 12.0),
                                egui::vec2(18.0, inner.height() - 20.0),
                            ),
                            Rounding::same(2.0),
                            egui::Color32::from_rgb(40, 34, 62),
                        );
                        let mut y = inner.top() + 13.0;
                        for w in [0.52_f32, 0.38, 0.46] {
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(inner.left() + 7.0, y),
                                    egui::vec2((inner.width() - 44.0) * w, 3.0),
                                ),
                                Rounding::same(1.5),
                                egui::Color32::from_rgb(38, 38, 45),
                            );
                            y += 9.0;
                        }
                    }

                    if running {
                        let pill = egui::Rect::from_min_size(
                            rect.left_top() + egui::vec2(6.0, 6.0),
                            egui::vec2(38.0, 15.0),
                        );
                        painter.rect_filled(
                            pill,
                            Rounding::same(4.0),
                            egui::Color32::from_black_alpha(170),
                        );
                        painter.circle_filled(
                            egui::pos2(pill.left() + 8.0, pill.center().y),
                            2.5,
                            t::LIVE,
                        );
                        painter.text(
                            egui::pos2(pill.left() + 14.0, pill.center().y),
                            egui::Align2::LEFT_CENTER,
                            "LIVE",
                            egui::FontId::proportional(7.5),
                            t::TEXT,
                        );
                    }
                    if resp.clicked() {
                        open_window = true;
                    }

                    ui.add_space(10.0);

                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("ТРАНСЛЯЦИЯ")
                                    .size(8.5)
                                    .strong()
                                    .color(t::TEXT_MUTED),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let fps = self
                                    .video
                                    .as_ref()
                                    .map(|v| v.fps().round() as u32)
                                    .unwrap_or_else(|| match self.config.video_fps {
                                        Some(30) => 30,
                                        _ => 60,
                                    });
                                let label = ui.label(
                                    egui::RichText::new(format!("{} FPS", fps))
                                        .size(8.0)
                                        .monospace()
                                        .color(t::TEXT_MUTED),
                                );
                                if let Some(v) = &self.video {
                                    let (d, r, i) = v.timings();
                                    label.on_hover_text(format!(
                                        "захват {} FPS\nдекод {:.1} · чтение GPU {:.1} · картинка {:.1} мс",
                                        v.encode_fps(),
                                        d,
                                        r,
                                        i
                                    ));
                                }
                            });
                        });

                        let title = self
                            .video
                            .as_ref()
                            .map(|v| v.source.clone())
                            .or_else(|| self.remote.as_ref().map(|r| format!("Смотрит: {}", r.peer)))
                            .unwrap_or_else(|| "Экран 1".to_string());
                        ui.label(
                            egui::RichText::new(title)
                                .size(12.0)
                                .strong()
                                .color(if running { t::TEXT } else { t::TEXT_DIM }),
                        );

                        let subtitle = if let Some(v) = &self.video {
                            let (w, h) = v.size();
                            let viewers = self
                                .video_link
                                .as_ref()
                                .map(|l| l.viewer_count())
                                .unwrap_or(0);
                            format!(
                                "{}×{} · {} FPS · {} зрит.",
                                w,
                                h,
                                v.encode_fps(),
                                viewers
                            )
                        } else {
                            "Не запущена".to_string()
                        };
                        ui.label(
                            egui::RichText::new(subtitle)
                                .size(8.0)
                                .color(t::TEXT_MUTED),
                        );

                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let label = if running { "Стоп" } else { "Запустить" };
                            if t::chip_button(ui, label, running).clicked() {
                                if running {
                                    self.video = None;
                                    self.video_fullscreen = false;
                                    if let Some(link) = &self.video_link {
                                        link.end_stream();
                                    }
                                } else {
                                    let fps = match self.config.video_fps {
                                        Some(30) => 30,
                                        _ => 60,
                                    };
                                    match video_session::VideoSession::start(fps) {
                                        Ok(v) => {
                                            // Адаптация битрейта: потери канала
                                            // feed будут опускать целевой
                                            // битрейт энкодера и поднимать назад.
                                            if let Some(link) = &self.video_link {
                                                link.set_bitrate_control(
                                                    v.bitrate_target(),
                                                    v.bitrate_base(),
                                                );
                                            }
                                            self.video = Some(v);
                                            self.video_error = None;
                                            // Анонс новой трансляции: зрители
                                            // найдут её в каталоге и подпишутся
                                            // сами, в том числе вошедшие позже.
                                            if let Some(link) = &self.video_link {
                                                link.begin_stream();
                                            }
                                        }
                                        Err(e) => self.video_error = Some(e),
                                    }
                                }
                            }

                            if self.remote.is_some()
                                && t::chip_button(ui, "Смотреть", false).clicked()
                            {
                                open_window = true;
                            }

                            if let Some(err) = self.video_error.clone() {
                                ui.label(egui::RichText::new(err).size(8.0).color(t::REC));
                            }
                        });
                    });
                });
            });

        if open_window {
            self.video_window = true;
        }
    }

    #[cfg(not(windows))]
    pub(super) fn draw_stream_card(&mut self, _ui: &mut egui::Ui) {}

    /// Микшер: микрофон, гитара и громкость собеседников
    pub(super) fn draw_mixer_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(t::INNER)
            .rounding(Rounding::same(10.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("ГРОМКОСТЬ")
                            .size(8.5)
                            .strong()
                            .color(t::TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Как в макете: значок микшера вместо подписи
                        if t::icon_button(ui, t::Icon::Sliders, 18.0, false)
                            .on_hover_text("Настройки звука")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                    });
                });
                ui.add_space(2.0);

                let mut mic = self.mixer.ch1_volume();
                let mut guitar = self.mixer.ch2_volume();
                let mut peers = self.mixer.output_volume();

                // Подпись — номер входа: профиль обработки живёт в настройках,
                // а по громкости видно, что за вход перед вами.
                mixer_row(ui, "ВХОД 1", &mut mic, self.mixer.input1_peak());
                mixer_row(ui, "ВХОД 2", &mut guitar, self.mixer.input2_peak());
                mixer_row(ui, "СОБЕСЕДНИКИ", &mut peers, 0.0);

                self.mixer.set_ch1_volume(mic);
                self.mixer.set_ch2_volume(guitar);
                self.mixer.set_output_volume(peers);
            });
    }
}
