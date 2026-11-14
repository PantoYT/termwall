//! The music for the page's visualizer: what plays on the default output device, as the 128 values
//! Wallpaper Engine hands a web wallpaper (64 bands per channel, bass first, left then right).
//!
//! Wallpaper Engine doesn't document where its 64 bands lie. The page was tuned against data in
//! which the bass fills the first bands and the top dozen stay empty, which is what equal-width
//! bands up to 20 kHz give, so that is the layout here (unverified against WE with a test tone).
//! The level follows the music (an automatic gain), so a quiet system volume still moves the bars.

use std::f32::consts::PI;

pub const BANDS: usize = 64;
pub const WINDOW: usize = 2048;
const TOP_HZ: f32 = 20_000.0;
/// Below this amplitude (about -50 dB of full scale) the gain stops rising: noise stays noise.
const AGC_FLOOR: f32 = 0.003;
/// How fast the remembered peak falls, per frame (30 frames a second: half in about 45 s).
const AGC_DECAY: f32 = 0.9995;

/// In-place radix-2 FFT; the length is a power of two.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
}

pub struct Spectrum {
    hann: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    agc: f32,
}

impl Default for Spectrum {
    fn default() -> Self {
        let hann = (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / WINDOW as f32).cos()).collect();
        Spectrum { hann, re: vec![0.0; WINDOW], im: vec![0.0; WINDOW], agc: 0.0 }
    }
}

impl Spectrum {
    /// The last WINDOW samples of one channel as 64 band amplitudes (a full-scale sine reads 1.0).
    fn channel(&mut self, samples: &[f32], rate: u32, out: &mut [f32]) {
        for i in 0..WINDOW {
            self.re[i] = samples.get(i).copied().unwrap_or(0.0) * self.hann[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        let bins_per_band = TOP_HZ / BANDS as f32 / (rate.max(1) as f32 / WINDOW as f32);
        let scale = 4.0 / WINDOW as f32; // 2 / the Hann window's sum
        for (b, slot) in out.iter_mut().enumerate().take(BANDS) {
            let lo = ((b as f32 * bins_per_band).ceil() as usize).max(1); // bin 0 is DC
            let hi = (((b + 1) as f32 * bins_per_band).ceil() as usize).min(WINDOW / 2);
            let mut peak = 0.0f32;
            for k in lo..hi.max(lo) {
                peak = peak.max(self.re[k] * self.re[k] + self.im[k] * self.im[k]);
            }
            *slot = peak.sqrt() * scale;
        }
    }

    /// Both channels' last WINDOW samples -> the 128 values for the page.
    pub fn frame(&mut self, left: &[f32], right: &[f32], rate: u32) -> [f32; BANDS * 2] {
        let mut out = [0.0f32; BANDS * 2];
        let (l, r) = out.split_at_mut(BANDS);
        self.channel(left, rate, l);
        self.channel(right, rate, r);
        let peak = out.iter().cloned().fold(0.0f32, f32::max);
        self.agc = peak.max(self.agc * AGC_DECAY);
        let gain = 0.9 / self.agc.max(AGC_FLOOR);
        for v in &mut out {
            *v *= gain;
        }
        out
    }
}

/// `window.__termwallHost.audio([...])` for one frame.
pub fn script(frame: &[f32; BANDS * 2]) -> String {
    let mut s = String::with_capacity(900);
    s.push_str("window.__termwallHost&&window.__termwallHost.audio([");
    for (i, v) in frame.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        if *v < 0.0005 {
            s.push('0');
        } else {
            s.push_str(&format!("{v:.3}"));
        }
    }
    s.push_str("])");
    s
}

#[cfg(windows)]
pub mod capture {
    //! WASAPI loopback: the mix going to a playback device, read by polling (no event handle:
    //! loopback streams don't signal one on older Windows 10 builds).
    //!
    //! Which device: the default one while it carries sound. When it is silent and another one
    //! plays, that one (the loudest). Mixers such as SteelSeries Sonar or VoiceMeeter spread
    //! programs over several virtual devices, and the music is often not on the default one.

    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE,
        IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
        WAVEFORMATEXTENSIBLE, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::core::GUID;

    use super::{BANDS, Spectrum, WINDOW};

    const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

    #[derive(Clone, Copy)]
    struct Format {
        channels: usize,
        rate: u32,
        bytes: usize,
        float: bool,
    }

    impl Format {
        /// SAFETY: p points at the mix format WASAPI returned.
        unsafe fn read(p: *const WAVEFORMATEX) -> Format {
            let f = unsafe { p.read_unaligned() };
            let mut float = f.wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
            if f.wFormatTag == WAVE_FORMAT_EXTENSIBLE && f.cbSize >= 22 {
                let ext = unsafe { (p as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
                let sub = ext.SubFormat; // copied out: the struct is packed
                float = sub == SUBTYPE_IEEE_FLOAT;
            }
            Format { channels: f.nChannels as usize, rate: f.nSamplesPerSec, bytes: (f.wBitsPerSample / 8) as usize, float }
        }

        fn ok(&self) -> bool {
            self.channels >= 1 && (if self.float { self.bytes == 4 } else { (2..=4).contains(&self.bytes) })
        }

        /// One sample at this byte offset, as -1.0..1.0.
        fn sample(&self, b: &[u8]) -> f32 {
            match (self.float, self.bytes) {
                (true, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                (false, 2) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
                (false, 3) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
                _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            }
        }
    }

    fn device_id(device: &IMMDevice) -> windows::core::Result<String> {
        // SAFETY: GetId hands out a CoTaskMem string, freed right after it is copied.
        unsafe {
            let p = device.GetId()?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            Ok(s)
        }
    }

    /// "Headphones (Some Headset)" for an endpoint id, from where Windows keeps the names.
    fn device_name(id: &str) -> String {
        let guid = id.rsplit('.').next().unwrap_or(id);
        let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render\{guid}\Properties");
        let read = |name: &str| crate::platform::reg::read(crate::platform::reg::HKLM, &key, name).filter(|s| !s.is_empty());
        match (read("{a45c254e-df1c-4efd-8020-67d146a850e0},2"), read("{b3f8fa53-0004-438e-9003-51a46e139bfc},6")) {
            (Some(n), Some(i)) => format!("{n} ({i})"),
            (Some(n), None) => n,
            _ => id.to_string(),
        }
    }

    /// The device to listen on: the default one if it plays, else the loudest one that does,
    /// else the default one. The meters are read for a quarter of a second (a beat's gap is
    /// not silence).
    /// SAFETY: COM calls on this thread's own interfaces.
    unsafe fn pick(enumerator: &IMMDeviceEnumerator) -> windows::core::Result<IMMDevice> {
        unsafe {
            let default = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let all = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut devices = vec![default.clone()];
            for i in 0..all.GetCount()? {
                devices.push(all.Item(i)?);
            }
            let meters: Vec<Option<IAudioMeterInformation>> = devices.iter().map(|d| d.Activate(CLSCTX_ALL, None).ok()).collect();
            let mut peaks = vec![0.0f32; devices.len()];
            for round in 0..6 {
                if round > 0 {
                    std::thread::sleep(Duration::from_millis(45));
                }
                for (peak, meter) in peaks.iter_mut().zip(&meters) {
                    if let Some(v) = meter.as_ref().and_then(|m| m.GetPeakValue().ok()) {
                        *peak = peak.max(v);
                    }
                }
            }
            const PLAYS: f32 = 0.001;
            let loudest = (1..devices.len()).max_by(|a, b| peaks[*a].total_cmp(&peaks[*b]));
            Ok(match loudest {
                Some(i) if peaks[0] <= PLAYS && peaks[i] > PLAYS => devices.swap_remove(i),
                _ => default,
            })
        }
    }

    /// Capture on a thread of its own for as long as `wanted()` says so (asked every two seconds:
    /// no visualizer on any screen means no capture at all). `on_frame` gets about 30 frames a
    /// second while something plays and two a second of zeros while nothing does.
    pub fn spawn(wanted: impl Fn() -> bool + Send + 'static, on_frame: impl Fn(&[f32; BANDS * 2]) + Send + 'static) {
        let run = move || {
            // SAFETY: COM for this thread only; the interfaces never leave it.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let (mut said, mut device) = (String::new(), String::new());
            loop {
                if wanted() {
                    // SAFETY: see capture_one.
                    if let Err(e) = unsafe { capture_one(&wanted, &on_frame, &mut device) } {
                        let msg = e.to_string();
                        if msg != said {
                            eprintln!("audio: {msg}");
                            said = msg;
                        }
                        std::thread::sleep(Duration::from_secs(2));
                    }
                } else {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        };
        let _ = std::thread::Builder::new().name("termwall-audio".into()).spawn(run);
    }

    /// Listens on one device until another one should be listened on instead (see `pick`), the
    /// visualizer is switched off, or WASAPI fails. `listening` is the device's id, kept between
    /// calls so that a change is said once.
    /// SAFETY: plain COM calls on interfaces owned by this function; buffers are read within the
    /// bounds WASAPI reports and released before the next call.
    unsafe fn capture_one(
        wanted: &dyn Fn() -> bool,
        on_frame: &dyn Fn(&[f32; BANDS * 2]),
        listening: &mut String,
    ) -> windows::core::Result<()> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = pick(&enumerator)?;
            let id = device_id(&device)?;
            if *listening != id {
                println!("audio: listening on {}", device_name(&id));
                *listening = id.clone();
            }
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let pwfx = client.GetMixFormat()?;
            let fmt = Format::read(pwfx);
            let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, pwfx, None);
            CoTaskMemFree(Some(pwfx as *const _));
            init?;
            if !fmt.ok() {
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(0x8000_4001u32 as i32),
                    format!("unsupported mix format ({} bytes a sample, float: {})", fmt.bytes, fmt.float),
                ));
            }
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;

            let mut spectrum = Spectrum::default();
            let mut left: VecDeque<f32> = std::iter::repeat_n(0.0, WINDOW).collect();
            let mut right = left.clone();
            let frame_bytes = fmt.channels * fmt.bytes;
            let start = Instant::now();
            let (mut last_packet, mut last_emit, mut last_sent, mut last_check) = (None::<Instant>, start, start, start);
            let mut heard = start; // when this device last carried sound
            loop {
                while capture.GetNextPacketSize()? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                    capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                    let bytes: &[u8] = if silent { &[] } else { std::slice::from_raw_parts(data, frames as usize * frame_bytes) };
                    for f in 0..frames as usize {
                        let (l, r) = if silent {
                            (0.0, 0.0)
                        } else {
                            let at = f * frame_bytes;
                            let l = fmt.sample(&bytes[at..]);
                            (l, if fmt.channels > 1 { fmt.sample(&bytes[at + fmt.bytes..]) } else { l })
                        };
                        left.pop_front();
                        left.push_back(l);
                        right.pop_front();
                        right.push_back(r);
                    }
                    capture.ReleaseBuffer(frames)?;
                    last_packet = Some(Instant::now());
                }
                let now = Instant::now();
                if now.duration_since(last_emit) >= Duration::from_millis(33) {
                    last_emit = now;
                    // nothing plays: WASAPI sends no packets, and the old samples must not linger
                    let live = last_packet.is_some_and(|t| now.duration_since(t) < Duration::from_millis(200));
                    let frame = if live {
                        spectrum.frame(left.make_contiguous(), right.make_contiguous(), fmt.rate)
                    } else {
                        [0.0; BANDS * 2]
                    };
                    let loud = frame.iter().any(|v| *v > 0.001);
                    if loud {
                        heard = now;
                    }
                    if loud || now.duration_since(last_sent) >= Duration::from_millis(500) {
                        last_sent = now;
                        on_frame(&frame);
                    }
                }
                if now.duration_since(last_check) >= Duration::from_secs(2) {
                    last_check = now;
                    if !wanted() {
                        break;
                    }
                    // quiet here for two seconds: is the music (now) somewhere else?
                    if now.duration_since(heard) >= Duration::from_secs(2) && device_id(&pick(&enumerator)?)? != id {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = client.Stop();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amp: f32, rate: u32) -> Vec<f32> {
        (0..WINDOW).map(|i| amp * (2.0 * PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn fft_finds_a_tone() {
        let (mut re, mut im) = (sine(48_000.0 * 64.0 / WINDOW as f32, 1.0, 48_000), vec![0.0; WINDOW]);
        fft(&mut re, &mut im);
        let mag: Vec<f32> = re.iter().zip(&im).map(|(a, b)| (a * a + b * b).sqrt()).collect();
        let top = (0..WINDOW / 2).max_by(|a, b| mag[*a].total_cmp(&mag[*b])).unwrap();
        assert_eq!(top, 64, "a tone on bin 64 peaks there");
        assert!((mag[64] - WINDOW as f32 / 2.0).abs() < 1.0, "with half the length as its magnitude: {}", mag[64]);
        assert!(mag[10] < 0.01 && mag[500] < 0.01, "and nothing elsewhere");
    }

    #[test]
    fn bands_from_bass_up_left_then_right() {
        let mut s = Spectrum::default();
        let silence = vec![0.0; WINDOW];
        assert!(s.frame(&silence, &silence, 48_000).iter().all(|v| *v == 0.0), "silence is all zeros");
        // 1 kHz on the left only: 20 kHz / 64 bands = 312.5 Hz a band, so band 3
        let f = s.frame(&sine(1000.0, 0.5, 48_000), &silence, 48_000);
        let top = (0..128).max_by(|a, b| f[*a].total_cmp(&f[*b])).unwrap();
        assert_eq!(top, 3, "1 kHz is band 3 of the left channel");
        assert!(f[64..].iter().all(|v| *v < 0.01), "the right channel stays empty");
        assert!((f[3] - 0.9).abs() < 0.05, "the gain brings the loudest band to about 0.9: {}", f[3]);
        // the same tone forty times quieter still reads high: the level follows the music...
        let mut q = Spectrum::default();
        let quiet = q.frame(&silence, &sine(100.0, 0.0125, 44_100), 44_100);
        assert!(quiet[64] > 0.8 && quiet[..64].iter().all(|v| *v < 0.01), "100 Hz on the right is band 64: {}", quiet[64]);
        // ...but noise near the floor is not blown up to full height
        let mut n = Spectrum::default();
        assert!(n.frame(&sine(1000.0, 0.0005, 48_000), &silence, 48_000)[3] < 0.2, "near-silence stays low");
    }

    #[test]
    fn the_script_for_the_page() {
        let mut f = [0.0f32; 128];
        f[0] = 0.5;
        f[127] = 1.25;
        let s = script(&f);
        assert!(s.starts_with("window.__termwallHost&&window.__termwallHost.audio([0.500,0,") && s.ends_with(",1.250])"));
        assert_eq!(s.matches(',').count(), 127);
    }
}
