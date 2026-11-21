//! Сохранение настроек на диск: только изменённое и не чаще 400 мс.

use super::*;
use std::time::{Duration, Instant};

impl RadioApp {
    /// Сохраняет настройки на диск не чаще раза в 400 мс после изменений
    pub(super) fn persist_if_needed(&mut self) {
        let mut changed = false;

        let ch1 = self.mixer.ch1_volume();
        if self.config.ch1_volume != Some(ch1) {
            self.config.ch1_volume = Some(ch1);
            changed = true;
        }
        let ch2 = self.mixer.ch2_volume();
        if self.config.ch2_volume != Some(ch2) {
            self.config.ch2_volume = Some(ch2);
            changed = true;
        }
        let out = self.mixer.output_volume();
        if self.config.output_volume != Some(out) {
            self.config.output_volume = Some(out);
            changed = true;
        }
        let in1 = self.mixer.input1_channel();
        if self.config.input1_channel != Some(in1) {
            self.config.input1_channel = Some(in1);
            changed = true;
        }
        let in2 = self.mixer.input2_channel();
        if self.config.input2_channel != Some(in2) {
            self.config.input2_channel = Some(in2);
            changed = true;
        }
        // Профиль — пресет тумблеров; в файл пишем сами тумблеры, а старый
        // ключ профиля — только пока он однозначен (для старых exe).
        let profile1 = self.mixer.input1_profile();
        let legacy1 = match profile1 {
            crate::mixer::SlotProfile::Custom => None,
            p => Some(p.as_u32()),
        };
        if self.config.input1_profile != legacy1 {
            self.config.input1_profile = legacy1;
            changed = true;
        }
        let profile2 = self.mixer.input2_profile();
        let legacy2 = match profile2 {
            crate::mixer::SlotProfile::Custom => None,
            p => Some(p.as_u32()),
        };
        if self.config.input2_profile != legacy2 {
            self.config.input2_profile = legacy2;
            changed = true;
        }
        for (key, value) in [
            ("input1_denoise", self.mixer.input1_denoise()),
            ("input2_denoise", self.mixer.input2_denoise()),
            ("input1_aec", self.mixer.input1_aec()),
            ("input2_aec", self.mixer.input2_aec()),
            ("input1_gate", self.mixer.input1_gate()),
            ("input1_agc", self.mixer.input1_agc()),
            ("input2_gate", self.mixer.input2_gate()),
            ("input2_agc", self.mixer.input2_agc()),
        ] {
            let slot = match key {
                "input1_denoise" => &mut self.config.input1_denoise,
                "input2_denoise" => &mut self.config.input2_denoise,
                "input1_aec" => &mut self.config.input1_aec,
                "input2_aec" => &mut self.config.input2_aec,
                "input1_gate" => &mut self.config.input1_gate,
                "input1_agc" => &mut self.config.input1_agc,
                "input2_gate" => &mut self.config.input2_gate,
                _ => &mut self.config.input2_agc,
            };
            if *slot != Some(value) {
                *slot = Some(value);
                changed = true;
            }
        }
        let bitrate = self.mixer.opus_bitrate();
        if self.config.opus_bitrate != Some(bitrate) {
            self.config.opus_bitrate = Some(bitrate);
            changed = true;
        }
        if self.config.continuous_mode != Some(self.continuous_mode) {
            self.config.continuous_mode = Some(self.continuous_mode);
            changed = true;
        }
        let vk = crate::keybind::CURRENT_VK.load(std::sync::atomic::Ordering::Relaxed);
        if self.config.ptt_vk != Some(vk) {
            self.config.ptt_vk = Some(vk);
            changed = true;
        }
        if self.config.nick.as_deref() != Some(self.nick.as_str()) {
            self.config.nick = Some(self.nick.clone());
            changed = true;
        }
        let tx_limiter = self.mixer.is_tx_limiter_enabled();
        if self.config.tx_limiter != Some(tx_limiter) {
            self.config.tx_limiter = Some(tx_limiter);
            changed = true;
        }
        // Старый общий тумблер — «есть ли хоть один эффект»: так его поймёт
        // exe прежней версии, а наш загрузчик его проигнорирует, если есть
        // новые ключи.
        let legacy_dsp = self.mixer.input1_denoise()
            || self.mixer.input1_gate()
            || self.mixer.input1_agc()
            || self.mixer.input2_denoise()
            || self.mixer.input2_gate()
            || self.mixer.input2_agc();
        if self.config.mic_processing != Some(legacy_dsp) {
            self.config.mic_processing = Some(legacy_dsp);
            changed = true;
        }
        let rx_limiter = self.mixer.is_rx_limiter_enabled();
        if self.config.rx_limiter != Some(rx_limiter) {
            self.config.rx_limiter = Some(rx_limiter);
            changed = true;
        }
        let peer_vols = self.mixer.peer_volumes();
        if self.config.peer_volumes != peer_vols {
            self.config.peer_volumes = peer_vols;
            changed = true;
        }

        if changed {
            self.pending_save = true;
        }

        if self.pending_save && self.last_save.elapsed() >= Duration::from_millis(400) {
            self.config.save();
            self.pending_save = false;
            self.last_save = Instant::now();
        }
    }
}
