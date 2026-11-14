//! Аппаратный H.264-декодер Windows (Media Foundation MFT).
//!
//! Вход — Annex-B кадры, выход — NV12 в системной памяти. Дальше
//! `win_d3d::Converter` переводит NV12 в BGRA на GPU для показа.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use super::publisher::StreamInfo;
use super::EncodedUnit;

const WAIT: Duration = Duration::from_secs(2);

enum Ev {
    NeedInput,
    HaveOutput,
    Other,
}

pub struct MfDecoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    output_info: MFT_OUTPUT_STREAM_INFO,
    frame_duration: i64,
    next_pts: i64,
    need_input: bool,
    width: u32,
    height: u32,
    fps: u32,
    pub mft_name: String,
    pub notes: Vec<String>,
}

impl MfDecoder {
    pub fn new(info: &StreamInfo) -> Result<Self, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0).map_err(|e| format!("MFStartup: {e}"))?;

            let activations = enumerate_h264_decoders()?;
            if activations.is_empty() {
                return Err("не найден H.264-декодер".into());
            }

            let mut last_err = String::new();
            for act in &activations {
                let name = activation_name(act).unwrap_or_else(|| "MFT".into());
                match Self::configure(act, info, name) {
                    Ok(dec) => return Ok(dec),
                    Err(e) => last_err = e,
                }
            }
            Err(format!("ни один декодер не настроился: {last_err}"))
        }
    }

    unsafe fn configure(
        act: &IMFActivate,
        info: &StreamInfo,
        name: String,
    ) -> Result<Self, String> {
        let transform: IMFTransform = act
            .ActivateObject()
            .map_err(|e| format!("{name}: ActivateObject: {e}"))?;

        let attrs = transform
            .GetAttributes()
            .map_err(|e| format!("{name}: GetAttributes: {e}"))?;
        attrs
            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
            .map_err(|e| format!("{name}: async unlock: {e}"))?;
        attrs
            .SetUINT32(&MF_LOW_LATENCY, 1)
            .map_err(|e| format!("{name}: low latency: {e}"))?;

        let in_type = MFCreateMediaType().map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(info.width, info.height))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(info.fps, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        // SPS/PPS: декодер должен знать их до первого кадра
        if !info.sequence_header.is_empty() {
            in_type
                .SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &info.sequence_header)
                .map_err(|e| format!("{name}: sequence header: {e}"))?;
        }
        transform
            .SetInputType(0, &in_type, 0)
            .map_err(|e| format!("{name}: SetInputType H264: {e}"))?;

        let out_type = MFCreateMediaType().map_err(|e| e.to_string())?;
        out_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        out_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(info.width, info.height))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(info.fps, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        transform
            .SetOutputType(0, &out_type, 0)
            .map_err(|e| format!("{name}: SetOutputType NV12: {e}"))?;

        let events: IMFMediaEventGenerator = transform
            .cast()
            .map_err(|e| format!("{name}: MFT не асинхронный: {e}"))?;
        let output_info = transform
            .GetOutputStreamInfo(0)
            .map_err(|e| format!("{name}: GetOutputStreamInfo: {e}"))?;

        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|e| e.to_string())?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            transform,
            events,
            output_info,
            frame_duration: 10_000_000 / info.fps.max(1) as i64,
            next_pts: 0,
            need_input: false,
            width: info.width,
            height: info.height,
            fps: info.fps,
            mft_name: name,
            notes: Vec::new(),
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Отдаёт один сжатый кадр, возвращает все декодированные NV12-кадры.
    pub fn push(&mut self, unit: &EncodedUnit) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        let deadline = Instant::now() + WAIT;

        while !self.need_input {
            match self.wait_event(deadline)? {
                Ev::NeedInput => self.need_input = true,
                Ev::HaveOutput => out.extend(self.pull_output()?),
                Ev::Other => {}
            }
        }
        self.send_input(unit)?;
        self.need_input = false;

        loop {
            match self.poll_event()? {
                Some(Ev::HaveOutput) => out.extend(self.pull_output()?),
                Some(Ev::NeedInput) => self.need_input = true,
                Some(Ev::Other) => {}
                None => break,
            }
        }
        Ok(out)
    }

    fn poll_event(&self) -> Result<Option<Ev>, String> {
        unsafe {
            match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(ev) => Ok(Some(Self::classify(&ev)?)),
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(None),
                Err(e) => Err(format!("GetEvent: {e}")),
            }
        }
    }

    fn wait_event(&self, deadline: Instant) -> Result<Ev, String> {
        loop {
            if let Some(ev) = self.poll_event()? {
                return Ok(ev);
            }
            if Instant::now() >= deadline {
                return Err("таймаут ожидания события декодера".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn classify(ev: &IMFMediaEvent) -> Result<Ev, String> {
        let t = unsafe { ev.GetType() }.map_err(|e| format!("GetType: {e}"))? as u32;
        Ok(if t == METransformNeedInput.0 as u32 {
            Ev::NeedInput
        } else if t == METransformHaveOutput.0 as u32 {
            Ev::HaveOutput
        } else {
            Ev::Other
        })
    }

    fn send_input(&mut self, unit: &EncodedUnit) -> Result<(), String> {
        unsafe {
            let buf = MFCreateMemoryBuffer(unit.data.len() as u32).map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock input: {e}"))?;
            std::ptr::copy_nonoverlapping(unit.data.as_ptr(), ptr, unit.data.len());
            buf.Unlock().map_err(|e| e.to_string())?;
            buf.SetCurrentLength(unit.data.len() as u32)
                .map_err(|e| e.to_string())?;

            // метки времени должны строго возрастать
            let pts = unit.pts_100ns.max(self.next_pts);
            self.next_pts = pts + self.frame_duration;

            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buf).map_err(|e| e.to_string())?;
            sample.SetSampleTime(pts).map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(self.frame_duration)
                .map_err(|e| e.to_string())?;

            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| format!("ProcessInput: {e}"))
        }
    }

    fn pull_output(&mut self) -> Result<Vec<Vec<u8>>, String> {
        unsafe {
            let provides_samples =
                self.output_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let sample = if provides_samples {
                None
            } else {
                let buf = MFCreateMemoryBuffer(self.output_info.cbSize.max(self.width * self.height * 3 / 2))
                    .map_err(|e| e.to_string())?;
                let s = MFCreateSample().map_err(|e| e.to_string())?;
                s.AddBuffer(&buf).map_err(|e| e.to_string())?;
                Some(s)
            };

            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            match self
                .transform
                .ProcessOutput(0, std::slice::from_mut(&mut data), &mut status)
            {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(Vec::new()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.apply_stream_change()?;
                    return Ok(Vec::new());
                }
                Err(e) => return Err(format!("ProcessOutput: {e}")),
            }

            let sample = ManuallyDrop::take(&mut data.pSample);
            let sample = match sample {
                Some(s) => s,
                None => return Ok(Vec::new()),
            };
            let buf = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock output: {e}"))?;
            let bytes = std::slice::from_raw_parts(ptr, current as usize).to_vec();
            buf.Unlock().map_err(|e| e.to_string())?;
            Ok(vec![bytes])
        }
    }

    fn apply_stream_change(&mut self) -> Result<(), String> {
        unsafe {
            let mut index = 0;
            loop {
                let Ok(t) = self.transform.GetOutputAvailableType(0, index) else {
                    break;
                };
                let _ = t.SetUINT64(&MF_MT_FRAME_SIZE, pack2(self.width, self.height));
                let _ = t.SetUINT64(&MF_MT_FRAME_RATE, pack2(self.fps, 1));
                if self.transform.SetOutputType(0, &t, 0).is_ok() {
                    break;
                }
                index += 1;
            }
            self.output_info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}

unsafe fn enumerate_h264_decoders() -> Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };

    let mut result = Vec::new();
    for flags in [
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        MFT_ENUM_FLAG_SORTANDFILTER,
        MFT_ENUM_FLAG_ALL,
    ] {
        let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            flags,
            Some(&input),
            Some(&output),
            &mut raw,
            &mut count,
        )
        .is_err()
        {
            continue;
        }
        if raw.is_null() || count == 0 {
            continue;
        }
        for i in 0..count as usize {
            if let Some(act) = (*raw.add(i)).take() {
                result.push(act);
            }
        }
        CoTaskMemFree(Some(raw as *const core::ffi::c_void));
        if !result.is_empty() {
            break;
        }
    }
    Ok(result)
}

unsafe fn activation_name(act: &IMFActivate) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = 0u32;
    act.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
        .ok()?;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
    Some(String::from_utf16_lossy(&buf[..end.min(buf.len())]))
}

fn pack2(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}
