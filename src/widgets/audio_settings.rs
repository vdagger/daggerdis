//! Настройки звука для отдельного окна

use crate::gui::theme as t;
use eframe::egui;

fn truncate_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() > max_chars {
        let mut truncated: String = name.chars().take(max_chars.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    } else {
        name.to_string()
    }
}

pub fn draw_audio_settings(
    ui: &mut eframe::egui::Ui,
    input_devices: &[String],
    input_channel_counts: &[u32],
    output_devices: &[String],
    selected_input: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    selected_output: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    mixer: &crate::mixer::AudioMixer,
    ptt_bind: &mut Option<crate::keybind::PttBind>,
) {
    let in_idx = selected_input.load(std::sync::atomic::Ordering::Relaxed) as usize;
    let in_count = input_channel_counts.get(in_idx).copied().unwrap_or(0) as usize;
    let full_in_name = input_devices
        .get(in_idx)
        .cloned()
        .unwrap_or_else(|| "Не выбрано".to_string());

    // Входное устройство
    t::section_label(ui, "ВХОДНОЕ УСТРОЙСТВО");
    ui.add_space(4.0);
    egui::ComboBox::from_id_source("input_device_combo")
        .width(ui.available_width())
        .selected_text(truncate_name(&full_in_name, 34))
        .show_ui(ui, |ui| {
            for (i, device) in input_devices.iter().enumerate() {
                let mut current = selected_input.load(std::sync::atomic::Ordering::Relaxed);
                if ui
                    .selectable_value(&mut current, i as u32, device)
                    .changed()
                {
                    selected_input.store(current, std::sync::atomic::Ordering::Relaxed);
                    println!("[АУДИО] Выбран микрофон: {}", device);
                }
            }
        });

    ui.add_space(14.0);

    // Вход 1
    input_block(
        ui,
        "МИКРОФОН",
        mixer.input1_peak(),
        mixer.ch1_volume(),
        |v| mixer.set_ch1_volume(v),
        mixer.input1_channel(),
        |c| mixer.set_input1_channel(c),
        in_count,
        "ch1",
    );

    ui.add_space(12.0);

    // Вход 2
    input_block(
        ui,
        "ГИТАРА",
        mixer.input2_peak(),
        mixer.ch2_volume(),
        |v| mixer.set_ch2_volume(v),
        mixer.input2_channel(),
        |c| mixer.set_input2_channel(c),
        in_count,
        "ch2",
    );

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(12.0);

    // Общая громкость собеседников
    t::section_label(ui, "ГРОМКОСТЬ СОБЕСЕДНИКОВ");
    ui.add_space(4.0);
    let mut out_vol = mixer.output_volume();
    ui.spacing_mut().slider_width = (ui.available_width() - 62.0).max(80.0);
    if ui
        .add(
            egui::Slider::new(&mut out_vol, 0..=300)
                .suffix("%")
                .show_value(true),
        )
        .changed()
    {
        mixer.set_output_volume(out_vol);
    }

    ui.add_space(16.0);

    // Выходное устройство
    t::section_label(ui, "ВЫХОДНОЕ УСТРОЙСТВО");
    ui.add_space(4.0);
    let out_idx = selected_output.load(std::sync::atomic::Ordering::Relaxed) as usize;
    let full_out_name = output_devices
        .get(out_idx)
        .cloned()
        .unwrap_or_else(|| "Не выбрано".to_string());
    egui::ComboBox::from_id_source("output_device_combo")
        .width(ui.available_width())
        .selected_text(truncate_name(&full_out_name, 34))
        .show_ui(ui, |ui| {
            for (i, device) in output_devices.iter().enumerate() {
                let mut current = selected_output.load(std::sync::atomic::Ordering::Relaxed);
                if ui
                    .selectable_value(&mut current, i as u32, device)
                    .changed()
                {
                    selected_output.store(current, std::sync::atomic::Ordering::Relaxed);
                    println!("[АУДИО] Выбраны динамики: {}", device);
                }
            }
        });

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(12.0);

    // Прослушивание своего тракта: баланс микрофона и гитары настраивается
    // на слух ещё до созвона — слышно то, что уйдёт собеседнику.
    t::section_label(ui, "ПРОСЛУШИВАНИЕ");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Слышать свой тракт")
                .size(11.5)
                .color(t::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut on = mixer.is_monitor_enabled();
            if t::switch(ui, &mut on).changed() {
                mixer.set_monitor_enabled(on);
            }
        });
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(
            "Гейт, AGC и лимитер — ровно то, что услышит собеседник. \
             Только в наушниках: через колонки микрофон поймает свой же голос.",
        )
        .size(9.5)
        .color(t::TEXT_MUTED),
    );

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(12.0);

    // Рация
    t::section_label(ui, "РАЦИЯ");
    ui.add_space(4.0);
    crate::keybind::handle_keybind_ui(ui, ptt_bind, ui.available_width());
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Работает поверх любого окна, в том числе в играх")
            .size(9.5)
            .color(t::TEXT_MUTED),
    );
}

fn input_block(
    ui: &mut egui::Ui,
    title: &str,
    level: f32,
    volume: u32,
    set_volume: impl Fn(u32),
    channel: u32,
    set_channel: impl Fn(u32),
    in_count: usize,
    id: &str,
) {
    ui.horizontal(|ui| {
        t::section_label(ui, title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 6.0), egui::Sense::hover());
            t::vu_meter(&ui.painter_at(rect), rect, level);
        });
    });

    ui.add_space(4.0);

    let mut value = volume;
    ui.spacing_mut().slider_width = (ui.available_width() - 52.0).max(80.0);
    if ui
        .add(egui::Slider::new(&mut value, 0..=100).show_value(true))
        .changed()
    {
        set_volume(value);
    }

    if in_count > 1 {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Канал")
                    .size(10.5)
                    .color(t::TEXT_MUTED),
            );
            let text = if (channel as usize) < in_count {
                format!("{}", channel + 1)
            } else {
                "Выключен".to_string()
            };
            egui::ComboBox::from_id_source(format!("{}_channel", id))
                .width(120.0)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for c in 0..in_count {
                        let mut current = channel;
                        if ui
                            .selectable_value(&mut current, c as u32, format!("{}", c + 1))
                            .changed()
                        {
                            set_channel(current);
                        }
                    }
                    let mut off = channel;
                    if ui.selectable_value(&mut off, u32::MAX, "Выключен").changed() {
                        set_channel(off);
                    }
                });
        });
    }
}
