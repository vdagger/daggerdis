//! Тема, тёмный заголовок и горячая смена аудиоустройств.

use super::*;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Rounding, Stroke};

impl RadioApp {
    /// Подменяет входные потоки, когда в настройках выбрали другие устройства.
    /// Второй поток существует только при отдельном устройстве; «То же
    /// устройство» снова собирает один поток с двух каналов первой карты.
    ///
    /// Сравнение идёт по именам устройств. Список обновляется на лету
    /// (`refresh_devices`), и после подключения карты прежний индекс начинает
    /// указывать на другое устройство: по индексу поток перезапускался бы на
    /// том же самом микрофоне и зря рвал бы звук.
    pub(super) fn reconcile_inputs(&mut self) {
        let current_input = self
            .selected_input
            .load(std::sync::atomic::Ordering::Relaxed);
        let current_input2 = self
            .selected_input2
            .load(std::sync::atomic::Ordering::Relaxed);

        let Some(device1) = self.input_devices.get(current_input as usize).cloned() else {
            return;
        };
        let device2 = if current_input2 == 0 {
            None
        } else {
            self.input_devices
                .get(current_input2 as usize - 1)
                .cloned()
        };
        if self.prev_input_name.as_deref() == Some(device1.as_str())
            && self.prev_input2_name == device2
        {
            return;
        }
        self.prev_input_name = Some(device1.clone());
        self.prev_input2_name = device2.clone();

        let separate = device2
            .as_deref()
            .map_or(false, |name| Some(name) != Some(device1.as_str()));

        self.input2_error = None;
        if separate {
            let device2 = device2.unwrap_or_default();
            // Первый поток с отдельным вторым устройством заполняет только
            // свой слот: иначе канал первой карты попал бы в обе дорожки.
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [true, false],
                Some(&device1),
                true,
            ) {
                self.show_toast(&format!("Вход 1 не открылся: {}", e));
            }
            // Второму входу микрофон по умолчанию не подставляем: в инструмент
            // молча пошёл бы не тот источник.
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream2,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [false, true],
                Some(&device2),
                false,
            ) {
                self.input2_error = Some(e);
            }
            self.config.input_device = Some(device1);
            self.config.input2_device = Some(device2);
        } else {
            // Сначала гасим второй поток, иначе он продолжит дописывать в слот 2
            // поверх первого и смесь станет вдвое громче.
            match self.mic_stream2.lock() {
                Ok(mut s) => *s = None,
                Err(p) => *p.into_inner() = None,
            }
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [true, true],
                Some(&device1),
                true,
            ) {
                self.show_toast(&format!("Микрофон не открылся: {}", e));
            }
            self.config.input_device = Some(device1);
            self.config.input2_device = None;
        }
        self.pending_save = true;
    }

    /// Забирает свежий список устройств, если фоновый обход увидел изменения.
    ///
    /// Горячее подключение (hot-plug): карту могут выдернуть или вставить при
    /// работающем приложении. Раньше список снимался один раз при старте, и
    /// новый USB-микрофон появлялся только после перезапуска, а выдернутая
    /// карта оставляла выбор на устройстве, которого уже нет.
    ///
    /// Выбор хранится по имени (в `config.ini` тоже), поэтому после обновления
    /// индексы пересчитываются по именам: «USB-микрофон» не превращается молча
    /// в «Стереомикшер», когда список сдвинулся. Если выбранного устройства
    /// больше нет — откатываемся на актуальное умолчание и говорим тостом,
    /// иначе пропажа звука выглядит беспричинной.
    pub(super) fn refresh_devices(&mut self) {
        let Some(snapshot) = self.device_watch.try_recv() else {
            return;
        };
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        let selected_in = self.selected_input.load(relaxed) as usize;
        let selected_in2 = self.selected_input2.load(relaxed) as usize;
        let selected_out = self.selected_output.load(relaxed) as usize;

        // План считает `devices::plan_device_update` — там же его правила и
        // тесты: пустой снимок не принимаем, выбор пересчитываем по имени,
        // пропавшее устройство откатываем.
        let Some(plan) = crate::devices::plan_device_update(
            &self.input_devices,
            &self.output_devices,
            selected_in,
            selected_in2,
            selected_out,
            snapshot,
        ) else {
            return;
        };

        // Журнал не только про выбранное устройство: «карта пропала, а звук
        // работает» — первый вопрос при разборе, и без этих строк непонятно,
        // видит ли приложение изменения вообще.
        if !plan.removed.is_empty() {
            eprintln!("[АУДИО] Устройства отключены: {}", plan.removed.join(", "));
        }
        if !plan.added.is_empty() {
            eprintln!("[АУДИО] Устройства подключены: {}", plan.added.join(", "));
        }
        if !plan.gone.is_empty() {
            eprintln!("[АУДИО] Выбранного устройства нет: {}", plan.gone.join(", "));
        }

        self.input_devices = plan.next.inputs;
        self.input_channel_counts = plan.next.input_channels;
        self.output_devices = plan.next.outputs;

        match plan.input {
            crate::devices::Reselection::Same(index) => {
                self.selected_input.store(index as u32, relaxed)
            }
            crate::devices::Reselection::Gone(fallback) => {
                self.selected_input.store(fallback.unwrap_or(0) as u32, relaxed)
            }
        }
        match plan.input2 {
            crate::devices::Reselection::Same(index) => {
                self.selected_input2.store(index as u32 + 1, relaxed)
            }
            crate::devices::Reselection::Gone(_) => self.selected_input2.store(0, relaxed),
        }
        match plan.output {
            crate::devices::Reselection::Same(index) => {
                self.selected_output.store(index as u32, relaxed)
            }
            crate::devices::Reselection::Gone(fallback) => {
                self.selected_output.store(fallback.unwrap_or(0) as u32, relaxed)
            }
        }

        if !plan.gone.is_empty() {
            self.show_toast(&format!("Устройство отключено: {}", plan.gone.join(", ")));
        } else if !plan.added.is_empty() {
            self.show_toast(&format!("Подключено: {}", plan.added.join(", ")));
        }
    }

    pub(super) fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.theme_configured {
            return;
        }

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = t::BG;
        visuals.window_fill = t::SURFACE;
        visuals.extreme_bg_color = t::INSET;
        visuals.faint_bg_color = t::SURFACE_HI;
        visuals.window_stroke = Stroke::new(1.0_f32, t::STROKE_SOFT);
        visuals.window_rounding = Rounding::same(16.0);
        visuals.menu_rounding = Rounding::same(10.0);
        visuals.window_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 16.0),
            blur: 44.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(170),
        };
        visuals.popup_shadow = visuals.window_shadow;

        visuals.widgets.inactive.bg_fill = t::with_alpha(egui::Color32::WHITE, 0.04);
        visuals.widgets.inactive.weak_bg_fill = t::SURFACE;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t::STROKE_SOFT);
        visuals.widgets.inactive.rounding = Rounding::same(8.0);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t::TEXT_DIM);

        visuals.widgets.hovered.bg_fill = t::with_alpha(egui::Color32::WHITE, 0.07);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.40));
        visuals.widgets.hovered.rounding = Rounding::same(8.0);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.widgets.active.bg_fill = t::with_alpha(t::BRAND, 0.20);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, t::BRAND);
        visuals.widgets.active.rounding = Rounding::same(8.0);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.selection.bg_fill = t::with_alpha(t::BRAND, 0.32);
        visuals.selection.stroke = Stroke::new(1.0_f32, t::TEXT);
        visuals.override_text_color = Some(t::TEXT);
        visuals.hyperlink_color = t::BRAND;
        visuals.slider_trailing_fill = true;

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(10.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(10.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(7.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.slider_width = 140.0;
        style.spacing.window_margin = egui::Margin::same(14.0);
        style.spacing.interact_size.y = 22.0;
        ctx.set_style(style);

        self.theme_configured = true;
    }

    /// Тёмный системный заголовок окна — иначе белая полоса ломает тёмный макет
    #[cfg(windows)]
    pub(super) fn apply_dark_titlebar(&mut self, frame: &eframe::Frame) {
        if self.title_dark {
            return;
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(w) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(w.hwnd.get() as *mut std::ffi::c_void);
        let on: i32 = 1;
        let res = unsafe {
            windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
                &on as *const i32 as *const std::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        self.title_dark = res.is_ok();
        // Трею нужен HWND, чтобы показывать окно, пока сам GUI спит.
        crate::tray::set_main_window(w.hwnd.get() as isize);
    }
}
