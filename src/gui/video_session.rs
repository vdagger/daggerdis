//! Локальная трансляция с превью: захват → H.264 → декод → текстура egui.
//!
//! Этот же путь будет использоваться для удалённого потока: сеть просто
//! подменит источник закодированных кадров.

use eframe::egui;

use crate::video::publisher::ScreenPublisher;
use crate::video::win_d3d::{
    create_device, nv12_texture, upload_nv12, Converter, D3dDevice, FORMAT_BGRA, FORMAT_NV12,
};
use crate::video::win_decode::MfDecoder;

pub struct VideoSession {
    publisher: ScreenPublisher,
    decoder: MfDecoder,
    d3d: D3dDevice,
    nv12_tex: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    converter: Converter,
    texture: Option<egui::TextureHandle>,
    width: u32,
    height: u32,
    frames: u64,
    pub source: String,
    pub last_error: Option<String>,
}

impl VideoSession {
    pub fn start() -> Result<Self, String> {
        let publisher = ScreenPublisher::start(60)?;
        let info = publisher.info.clone();

        let decoder = MfDecoder::new(&info)?;
        let d3d = create_device(None)?;
        let nv12_tex = nv12_texture(&d3d.device, info.width, info.height)?;
        let converter = Converter::new(
            &d3d.device,
            &d3d.context,
            FORMAT_NV12,
            info.width,
            info.height,
            FORMAT_BGRA,
            info.width,
            info.height,
            info.fps,
        )?;

        Ok(Self {
            source: publisher.source.clone(),
            publisher,
            decoder,
            d3d,
            nv12_tex,
            converter,
            texture: None,
            width: info.width,
            height: info.height,
            frames: 0,
            last_error: None,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Забирает готовые кадры, декодирует и обновляет текстуру.
    /// Не больше нескольких кадров за вызов, чтобы не подвешивать GUI.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut latest: Option<Vec<u8>> = None;
        let mut budget = 4;
        while budget > 0 {
            let unit = match self.publisher.rx.try_recv() {
                Ok(u) => u,
                Err(_) => break,
            };
            budget -= 1;
            match self.decoder.push(&unit) {
                Ok(frames) => {
                    if let Some(frame) = frames.into_iter().last() {
                        latest = Some(frame);
                        self.frames += 1;
                    }
                }
                Err(e) => {
                    self.last_error = Some(e);
                    break;
                }
            }
        }

        let Some(nv12) = latest else { return };
        upload_nv12(&self.d3d.context, &self.nv12_tex, self.width, &nv12);
        let bgra = match self.converter.convert_and_read(&self.nv12_tex) {
            Ok(b) => b,
            Err(e) => {
                self.last_error = Some(e);
                return;
            }
        };
        let image = bgra_to_image(&bgra, self.width, self.height);
        match &mut self.texture {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("video-preview", image, egui::TextureOptions::LINEAR));
            }
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
}

/// BGRA с GPU → ColorImage. egui ждёт RGBA, поэтому меняем B и R местами.
fn bgra_to_image(bgra: &[u8], width: u32, height: u32) -> egui::ColorImage {
    let pixels = bgra
        .chunks_exact(4)
        .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0]))
        .collect();
    egui::ColorImage {
        size: [width as usize, height as usize],
        pixels,
    }
}
