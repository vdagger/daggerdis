//! Карточка звонка поверх окна и выход из эфира.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Align, Layout, Order, Rounding, Stroke};

impl RadioApp {
    /// Карточка звонка поверх окна: входящий, «звоним…» или идущий разговор
    pub(super) fn draw_call_overlay(&mut self, ctx: &egui::Context) {
        let view = self.calls.view();
        if matches!(view, crate::calls::CallView::Idle) {
            return;
        }

        let screen = ctx.screen_rect();
        let width = 330.0_f32.min(screen.width() - 40.0);
        let pos = egui::pos2(screen.center().x - width * 0.5, screen.top() + 66.0);

        let mut accept = false;
        let mut decline = false;
        let mut cancel = false;
        let mut end = false;

        // Esc работает как крупная кнопка: выйти из звонка, не целясь мышью.
        // Но только когда фокус не в текстовом поле: там Esc — для текста,
        // иначе он заодно обрывал бы разговор при правке сообщения.
        // Enter — войти в комнату, не целясь мышью: приглашение часто приходит,
        // когда окно не в фокусе, а Esc рядом отклоняет.
        if !ctx.wants_keyboard_input()
            && matches!(view, crate::calls::CallView::Incoming { .. })
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
        {
            println!("[КОМНАТА] ENTER — входим по приглашению");
            accept = true;
        }
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            match view {
                crate::calls::CallView::Incoming { .. } => decline = true,
                crate::calls::CallView::Outgoing { .. } => cancel = true,
                crate::calls::CallView::Active { .. } => end = true,
                crate::calls::CallView::Idle => {}
            }
        }

        if let crate::calls::CallView::Active {
            members,
            peer,
            elapsed,
        } = &view
        {
            // В комнате на N подписью служит число участников; имя показываем,
            // когда собеседник один — как в прежнем личном звонке.
            let label = match (members, peer) {
                (2, Some(node)) => self
                    .friends
                    .friends()
                    .into_iter()
                    .find(|f| &f.node_id == node)
                    .map(|f| f.name)
                    .unwrap_or_else(|| crate::diagnostics::short_id(node)),
                _ => format!("{members} в комнате"),
            };
            // Идёт разговор: никакой карточки поверх окна — стрим и комната
            // должны оставаться видны. Только компактная плашка в углу.
            egui::Area::new(egui::Id::new("call_pill"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 66.0))
                .order(Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(t::SURFACE)
                        .rounding(Rounding::same(11.0))
                        .stroke(Stroke::new(1.0_f32, t::with_alpha(t::LIVE, 0.35)))
                        .shadow(egui::epaint::Shadow {
                            offset: egui::vec2(0.0, 8.0),
                            blur: 22.0,
                            spread: 0.0,
                            color: egui::Color32::from_black_alpha(130),
                        })
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(7.0, 7.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(dot.center(), 3.0, t::LIVE);
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(&label)
                                        .size(11.5)
                                        .strong()
                                        .color(t::TEXT),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(format_elapsed(*elapsed))
                                        .size(10.0)
                                        .monospace()
                                        .color(t::TEXT_MUTED),
                                );
                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Выйти")
                                                .size(10.0)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(t::REC)
                                        .rounding(Rounding::same(7.0)),
                                    )
                                    .on_hover_text("Выйти из комнаты (ESC)")
                                    .clicked()
                                {
                                    end = true;
                                }
                            });
                        });
                });
        } else {
        egui::Area::new(egui::Id::new("call_card"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(14.0))
                    .stroke(Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.45)))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 16.0),
                        blur: 42.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(160),
                    })
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);

                        match &view {
                            crate::calls::CallView::Idle => {}
                            crate::calls::CallView::Incoming { nick, .. } => {
                                ui.horizontal(|ui| {
                                    t::avatar(ui, nick, 34.0, true);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("Приглашение в комнату")
                                                .size(9.0)
                                                .strong()
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.label(
                                            egui::RichText::new(nick)
                                                .size(14.0)
                                                .strong()
                                                .color(t::TEXT),
                                        );
                                    });
                                });
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if t::chip_button(ui, "Отклонить", false).clicked() {
                                        decline = true;
                                    }
                                    if t::primary_button(ui, "Войти", egui::vec2(150.0, 30.0))
                                        .clicked()
                                    {
                                        accept = true;
                                    }
                                });
                                ui.add_space(4.0);
                                t::micro(ui, "ENTER — войти · ESC — отклонить");
                            }
                            crate::calls::CallView::Outgoing { nick, elapsed } => {
                                ui.horizontal(|ui| {
                                    t::avatar(ui, nick, 34.0, false);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("Приглашаем…")
                                                .size(9.0)
                                                .strong()
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.label(
                                            egui::RichText::new(nick)
                                                .size(14.0)
                                                .strong()
                                                .color(t::TEXT),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            egui::RichText::new(format_elapsed(*elapsed))
                                                .size(10.0)
                                                .monospace()
                                                .color(t::TEXT_MUTED),
                                        );
                                    });
                                });
                                ui.add_space(10.0);
                                let w = ui.available_width();
                                if ui
                                    .add_sized(
                                        egui::vec2(w, 30.0),
                                        egui::Button::new(
                                            egui::RichText::new("Отменить приглашение")
                                                .size(11.0)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(t::REC)
                                        .rounding(Rounding::same(8.0)),
                                    )
                                    .clicked()
                                {
                                    cancel = true;
                                }
                                ui.add_space(4.0);
                                t::micro(ui, "ESC — отменить приглашение");
                            }
                            // Идущий разговор рисуется плашкой выше, не карточкой
                            crate::calls::CallView::Active { .. } => {}
                        }
                    });
            });
        }

        if accept {
            self.calls.accept_incoming();
        }
        if decline {
            self.calls.decline_incoming();
        }
        if cancel {
            self.calls.cancel_outgoing();
        }
        if end {
            self.calls.end_active();
        }
        if accept || decline || cancel || end {
            self.mixer
                .set_ring_mode(self.calls.ring_mode());
        }
    }

    /// «Выйти из эфира»: закрыть звонок, отпустить всех собеседников и не
    /// пускать их обратно, пока не нажмут «Войти» или не позвонят снова.
    pub(super) fn leave_voice(&mut self) {
        // Объявляем уход до того, как разберём фазу звонка. Комната может быть
        // собрана одним mesh (без CallPhase::Active), и тогда «Выйти из эфира»
        // ничего не отправляло: собеседники держали нас в списке «молчит» до
        // таймаута и не могли пригласить обратно.
        self.calls.announce_leave();
        match self.calls.view() {
            crate::calls::CallView::Active { .. } => self.calls.end_active(),
            crate::calls::CallView::Outgoing { .. } => self.calls.cancel_outgoing(),
            crate::calls::CallView::Incoming { .. } => {
                self.calls.decline_incoming();
            }
            crate::calls::CallView::Idle => {}
        }
        self.mixer.set_ring_mode(self.calls.ring_mode());
        // Флаг «вышли» ставится до обрыва соединений: иначе их последний
        // IrohSync ещё успел бы вернуть собеседника в комнату.
        self.room.leave();
        // Видео вне комнаты не живёт: своя трансляция снимается с анонса,
        // чужая — снимается подписка и очищается очередь.
        #[cfg(windows)]
        if let Some(link) = &self.video_link {
            link.end_stream();
            link.unsubscribe();
        }
        #[cfg(windows)]
        {
            self.remote = None;
        }
        self.is_host.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(p2p) = self.iroh.clone() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                // Отказ/отмена/завершение шлются отдельной таской — даём им
                // уйти до обрыва, иначе собеседник ждёт таймаута вместо
                // мгновенного «звонок отклонён/завершён».
                handle.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    p2p.disconnect_all().await;
                });
            }
        }
        self.show_toast("Вы вышли из эфира");
    }
}
