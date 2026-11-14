//! Захват экрана через DXGI Desktop Duplication.
//!
//! Берём монитор, который раздаёт рабочий стол: перебираем адаптеры и их
//! выходы, пока DuplicateOutput не согласится. Кадр остаётся на GPU,
//! NV12 получаем через ID3D11VideoProcessor (конверсия и масштаб).

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::*;

use super::win_d3d::{create_device, Converter};
use super::VideoTarget;

pub struct ScreenCapture {
    #[allow(dead_code)]
    device: ID3D11Device,
    #[allow(dead_code)]
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    converter: Converter,
    out_width: u32,
    out_height: u32,
    /// Какой адаптер реально отдаёт экран (диагностика).
    pub adapter_name: String,
    pub source_width: u32,
    pub source_height: u32,
}

impl ScreenCapture {
    pub fn new(target: &VideoTarget) -> Result<Self, String> {
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| format!("CreateDXGIFactory1: {e}"))?;

            let mut adapter_index = 0;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(a) => a,
                    Err(_) => break,
                };
                adapter_index += 1;

                let adapter_name = adapter
                    .GetDesc()
                    .map(|d| {
                        let end = d
                            .Description
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(d.Description.len());
                        String::from_utf16_lossy(&d.Description[..end])
                    })
                    .unwrap_or_else(|_| "адаптер без имени".into());

                // устройство обязано быть на том же адаптере, что и выход
                let d3d = match create_device(Some(&adapter)) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let mut output_index = 0;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(o) => o,
                        Err(_) => break,
                    };
                    output_index += 1;

                    let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                        continue;
                    };
                    let Ok(duplication) = output1.DuplicateOutput(&d3d.device) else {
                        continue;
                    };
                    let desc = duplication.GetDesc();
                    let src_w = desc.ModeDesc.Width;
                    let src_h = desc.ModeDesc.Height;
                    if src_w == 0 || src_h == 0 {
                        continue;
                    }

                    let converter = Converter::new(
                        &d3d.device,
                        &d3d.context,
                        src_w,
                        src_h,
                        target.width,
                        target.height,
                        target.fps,
                    )?;

                    return Ok(Self {
                        device: d3d.device,
                        context: d3d.context,
                        duplication,
                        converter,
                        out_width: target.width,
                        out_height: target.height,
                        adapter_name,
                        source_width: src_w,
                        source_height: src_h,
                    });
                }
            }
            Err("не найден экран для захвата (дупликация недоступна)".into())
        }
    }

    /// Ждёт новый кадр и возвращает его в NV12. `Ok(None)` — за отведённое
    /// время экран не изменился.
    pub fn next_nv12(&mut self, timeout_ms: u32) -> Result<Option<Vec<u8>>, String> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match self
                .duplication
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
            {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(format!("AcquireNextFrame: {e}")),
            }

            let texture = resource
                .as_ref()
                .and_then(|r| r.cast::<ID3D11Texture2D>().ok());
            let result = match texture {
                Some(t) => self.converter.to_nv12(&t).map(Some),
                None => Ok(None),
            };
            // освобождать кадр надо до следующего AcquireNextFrame
            let _ = self.duplication.ReleaseFrame();
            result
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }
}
