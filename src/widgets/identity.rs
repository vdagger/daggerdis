//! Блок «Мой NodeID»: полный идентификатор и копирование в один клик.

use crate::gui::theme as t;
use eframe::egui;
use egui::{Color32, Rounding};

pub fn draw_my_node_id(ui: &mut egui::Ui, my_node_id: &str) {
    ui.horizontal(|ui| {
        t::section_label(ui, "МОЙ NODEID");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Копировать")
                            .size(11.0)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(t::BRAND_DEEP)
                    .rounding(Rounding::same(8.0)),
                )
                .clicked()
            {
                ui.output_mut(|o| o.copied_text = my_node_id.to_string());
            }
        });
    });

    ui.add_space(2.0);
    let label = ui.add(
        egui::Label::new(
            egui::RichText::new(my_node_id)
                .monospace()
                .size(10.5)
                .color(t::TEXT_DIM),
        )
        .wrap()
        .sense(egui::Sense::click()),
    );
    if label
        .on_hover_text("Нажмите, чтобы скопировать NodeID")
        .clicked()
    {
        ui.output_mut(|o| o.copied_text = my_node_id.to_string());
    }
}
