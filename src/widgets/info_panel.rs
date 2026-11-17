//! Верхняя панель: логотип, состояние сети, «Поделиться» и настройки

use crate::gui::theme as t;
use eframe::egui;
use egui::{Color32, Rounding, Stroke};

/// Знак Albus: кольцо с точкой, как в макете
fn draw_logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    painter.circle_filled(c, 12.5, Color32::from_rgb(26, 22, 40));
    painter.circle_stroke(c, 12.0, Stroke::new(1.2_f32, t::with_alpha(t::BRAND, 0.75)));
    painter.circle_stroke(c, 7.5, Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.35)));
    painter.circle_filled(c, 3.2, t::BRAND_2);
}

/// Плашка «Поделиться»: копирует NodeID в буфер
fn share_chip(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(104.0, 28.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        Rounding::same(8.0),
        if response.hovered() {
            egui::Color32::from_rgb(34, 34, 40)
        } else {
            egui::Color32::from_rgb(28, 28, 34)
        },
    );
    painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0_f32, t::STROKE));
    t::draw_icon(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        ),
        t::Icon::Copy,
        t::TEXT_DIM,
    );
    painter.text(
        egui::pos2(rect.left() + 30.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Поделиться",
        egui::FontId::proportional(10.5),
        t::TEXT_DIM,
    );
    response
}

fn avatar_me(ui: &mut egui::Ui, nick: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    // Плашка, как в макете, а не кружок
    painter.rect_filled(rect, Rounding::same(8.0), t::BRAND_2);
    painter.rect_stroke(
        rect,
        Rounding::same(8.0),
        Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.45)),
    );
    let initial = nick
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "Я".to_string());
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(12.0),
        Color32::WHITE,
    );
    response.on_hover_text(format!("Вы — {}", nick))
}

pub fn draw_top_bar(
    ui: &mut egui::Ui,
    my_node_id: &str,
    nick: &str,
    settings_open: &mut bool,
    copy: &mut bool,
) {
    ui.horizontal(|ui| {
        draw_logo(ui);
        ui.add_space(7.0);
        ui.vertical(|ui| {
            ui.add_space(1.0);
            ui.label(
                egui::RichText::new("ALBUS")
                    .size(13.0)
                    .strong()
                    .color(t::TEXT),
            );
            ui.label(
                egui::RichText::new("P2P RADIO")
                    .size(7.5)
                    .color(t::TEXT_MUTED),
            );
        });

        // Статус сети — по центру верхней панели
        let full = ui.available_width();
        ui.add_space((full * 0.5 - 78.0).max(8.0));
        // Без рамки, как в макете: точка и подпись
        let (pill, _) = ui.allocate_exact_size(egui::vec2(156.0, 24.0), egui::Sense::hover());
        let painter = ui.painter_at(pill);
        painter.circle_filled(egui::pos2(pill.left() + 14.0, pill.center().y), 3.0, t::LIVE);
        painter.text(
            egui::pos2(pill.left() + 23.0, pill.center().y),
            egui::Align2::LEFT_CENTER,
            "Сеть P2P · QUIC · E2E",
            egui::FontId::proportional(9.5),
            t::TEXT_DIM,
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = t::icon_button(ui, t::Icon::Settings, 28.0, *settings_open);
            if btn.clicked() {
                *settings_open = !*settings_open;
            }
            btn.on_hover_text("Настройки");

            ui.add_space(2.0);
            avatar_me(ui, nick);

            ui.add_space(2.0);
            if share_chip(ui)
                .on_hover_text(format!(
                    "Скопировать мой NodeID и отправить другу\n{}",
                    my_node_id
                ))
                .clicked()
            {
                ui.output_mut(|o| o.copied_text = my_node_id.to_string());
                *copy = true;
            }
        });
    });
}
