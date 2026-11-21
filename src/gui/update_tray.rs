//! Трей и «крестик» в `update`.
//!
//! Вынесено из `RadioApp::update` без изменения логики: команды из меню
//! иконки, закрытие окна в трей и опрос скрытого окна.

use super::*;

impl RadioApp {
    /// Команды трея и поведение «крестика».
    pub(super) fn update_tray(&mut self, ctx: &egui::Context) {
    if crate::tray::take_quit() {
        self.tray_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if crate::tray::take_show() {
        self.hidden_to_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    let root = ctx.viewport_id() == egui::ViewportId::ROOT;
    let wants_close =
        root && !self.tray_quit && ctx.input(|i| i.viewport().close_requested());
    if wants_close && self.config.close_to_tray.unwrap_or(true) {
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden_to_tray = true;
            crate::tray::notify(
                "Albus свёрнут в трей",
                "Приложение работает: эфир и звонки продолжаются",
            );
        }
    }
    if self.hidden_to_tray {
        // Скрытое окно просыпается только по запросу: без этого опроса
        // команда из меню иконки ждала бы следующего события.
        ctx.request_repaint_after(Duration::from_millis(200));
    }
    }
}
