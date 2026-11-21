//! Захват звука рабочего стола (WASAPI process loopback).
//!
//! Нам нужно то, что играет в колонках, **кроме звука нашего же приложения**:
//! иначе человек, который смотрит чужой стрим и говорит в микрофон, услышит
//! сам себя в «звуке стола». Поэтому берём не обычный loopback (он микширует
//! весь endpoint), а process loopback с исключением дерева наших процессов:
//! `ActivateAudioInterfaceAsync` с `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE`.
//!
//! Требуется Windows 10 2004+ (build 19041). Формат — 32-битный float, стерео,
//! 48 кГц; платформа отдаёт его нативно для process loopback.
//!
//! Модуль отдаёт кольцо сэмплов; кодированием и отправкой занимаются другие
//! слои (`network::desktop_audio`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{implement, Interface, Ref};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;

/// Частота и каналы: process loopback отдаёт float32, стерео.
pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Кадр 20 мс — как у голоса, чтобы Opus работал теми же кусками.
pub const FRAME_SAMPLES: usize = 960;
/// Сколько держим в кольце: 2 секунды на случай, если кодировщик притормозил.
const RING_SAMPLES: usize = RATE as usize * CHANNELS * 2;

/// `WAVE_FORMAT_IEEE_FLOAT`
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// Колбэк активации: нас интересует только «готово», результат забираем сами.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationDone {
    event: HANDLE,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationDone_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe {
            let _ = SetEvent(self.event);
        }
        Ok(())
    }
}

/// Захват звука стола: кольцо сэмплов `f32` (interleaved stereo).
pub struct DesktopCapture {
    ring: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// Сколько кадров пришло с нулями, потому что кольцо опустело.
    underruns: Arc<std::sync::atomic::AtomicU32>,
}

impl DesktopCapture {
    /// Поднимает поток захвата и ждёт, пока он настроится (до 5 с).
    pub fn start() -> Result<Self, String> {
        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(RING_SAMPLES)));
        let stop = Arc::new(AtomicBool::new(false));
        let underruns = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (r, s, u) = (ring.clone(), stop.clone(), underruns.clone());
        let handle = std::thread::Builder::new()
            .name("desktop-audio".into())
            .spawn(move || match run_capture(r, s, u, &ready_tx) {
                Ok(()) => {}
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| format!("поток захвата звука стола: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                ring,
                stop,
                handle: Some(handle),
                underruns,
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = handle.join();
                Err("захват звука стола не ответил за 5 с".into())
            }
        }
    }

    /// Ровно кадр 20 мс (стерео). Недобор добивается тишиной, чтобы Opus
    /// получал кадры фиксированного размера.
    pub fn read_frame(&self) -> Vec<f32> {
        let need = FRAME_SAMPLES * CHANNELS;
        let mut out = Vec::with_capacity(need);
        if let Ok(mut q) = self.ring.lock() {
            let take = need.min(q.len());
            out.extend(q.drain(..take));
        }
        if out.len() < need {
            self.underruns.fetch_add(1, Ordering::Relaxed);
            out.resize(need, 0.0);
        }
        out
    }

    /// Сколько кадров пришло с тишиной вместо звука (диагностика).
    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }
}

impl Drop for DesktopCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_capture(
    ring: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
    underruns: Arc<std::sync::atomic::AtomicU32>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let event = unsafe { CreateEventW(None, false, false, None) }
        .map_err(|e| format!("CreateEventW: {e}"))?;
    let client = match activate_excluding_self(event) {
        Ok(c) => c,
        Err(e) => {
            unsafe {
                let _ = CloseHandle(event);
            }
            return Err(e);
        }
    };
    let capture: IAudioCaptureClient = match unsafe { client.GetService() } {
        Ok(c) => c,
        Err(e) => {
            unsafe {
                let _ = CloseHandle(event);
            }
            return Err(format!("IAudioCaptureClient: {e}"));
        }
    };
    if let Err(e) = unsafe { client.Start() } {
        unsafe {
            let _ = CloseHandle(event);
        }
        return Err(format!("Start: {e}"));
    }
    let _ = ready.send(Ok(()));

    while !stop.load(Ordering::Relaxed) {
        let waited = unsafe { WaitForSingleObject(event, 100) };
        if waited.0 != 0 {
            continue;
        }
        loop {
            let frames = match unsafe { capture.GetNextPacketSize() } {
                Ok(n) => n,
                Err(_) => break,
            };
            if frames == 0 {
                break;
            }
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut got = 0u32;
            let mut flags = 0u32;
            if unsafe { capture.GetBuffer(&mut ptr, &mut got, &mut flags, None, None) }.is_err() {
                break;
            }
            let samples = got as usize * CHANNELS;
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            if let Ok(mut q) = ring.lock() {
                if silent || ptr.is_null() {
                    q.extend(std::iter::repeat_n(0.0, samples));
                } else {
                    let data = unsafe { std::slice::from_raw_parts(ptr as *const f32, samples) };
                    q.extend(data.iter().copied());
                }
                while q.len() > RING_SAMPLES {
                    q.pop_front();
                }
            }
            let _ = unsafe { capture.ReleaseBuffer(got) };
        }
    }
    unsafe {
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    let _ = underruns;
    Ok(())
}

/// Активирует process-loopback клиент, исключая наше дерево процессов.
fn activate_excluding_self(event: HANDLE) -> Result<IAudioClient, String> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: std::process::id(),
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // `ActivateAudioInterfaceAsync` ждёт параметры блобом в PROPVARIANT.
    let mut prop: PROPVARIANT = unsafe { std::mem::zeroed() };
    unsafe {
        let inner = &mut *prop.Anonymous.Anonymous;
        inner.vt = VT_BLOB;
        // Блоб выделяем в памяти COM: Windows может очистить PROPVARIANT
        // (PropVariantClear) после разбора, и указатель на стек тогда
        // освобождался бы как блок кучи — это и давало порчу кучи.
        let bytes = std::slice::from_raw_parts(
            &params as *const _ as *const u8,
            std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>(),
        );
        let blob = windows::Win32::System::Com::CoTaskMemAlloc(bytes.len());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), blob as *mut u8, bytes.len());
        inner.Anonymous.blob = BLOB {
            cbSize: bytes.len() as u32,
            pBlobData: blob as *mut u8,
        };
    }
    // Обработчик завершения обязателен: с NULL Windows падает.
    let handler: IActivateAudioInterfaceCompletionHandler = ActivationDone { event }.into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&prop),
            &handler,
        )
        .map_err(|e| format!("ActivateAudioInterfaceAsync: {e}"))?
    };
    let mut iface: Option<windows::core::IUnknown> = None;
    for _ in 0..300 {
        let mut hr = windows::core::HRESULT(0);
        let mut candidate: Option<windows::core::IUnknown> = None;
        let got = unsafe { operation.GetActivateResult(&mut hr, &mut candidate) };
        if got.is_ok() && hr.is_ok() {
            iface = candidate;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let iface = iface.ok_or_else(|| "активация звука стола не завершилась за 3 с".to_string())?;
    let client: IAudioClient = iface
        .cast()
        .map_err(|e| format!("IAudioClient: {e}"))?;

    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS as u16,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * (CHANNELS as u32) * 4,
        nBlockAlign: (CHANNELS as u16) * 4,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                0,
                0,
                &format,
                None,
            )
            .map_err(|e| format!("IAudioClient::Initialize: {e}"))?;
        client
            .SetEventHandle(event)
            .map_err(|e| format!("SetEventHandle: {e}"))?;
    }
    Ok(client)
}

/// Доля RMS и пик по кадру — для диагностики и проверок.
pub fn frame_level(frame: &[f32]) -> (f32, f32) {
    if frame.is_empty() {
        return (0.0, 0.0);
    }
    let sum: f32 = frame.iter().map(|s| s * s).sum();
    let rms = (sum / frame.len() as f32).sqrt();
    let peak = frame.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    (rms, peak)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_level_sees_quiet_and_loud() {
        let quiet = vec![0.0f32; 8];
        assert_eq!(frame_level(&quiet), (0.0, 0.0));
        let loud = vec![0.5f32, -0.5, 0.5, -0.5];
        let (rms, peak) = frame_level(&loud);
        assert!((rms - 0.5).abs() < 1e-6);
        assert!((peak - 0.5).abs() < 1e-6);
    }
}
