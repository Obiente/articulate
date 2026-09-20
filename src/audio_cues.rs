//! Small, generated PCM cues, played asynchronously through the Windows sound
//! device. No media downloads or files. Static storage outlives async playback.
#[derive(Clone, Copy)]
pub enum Cue {
    Started,
    Stopped,
}

#[cfg(all(windows, not(test)))]
pub fn play(cue: Cue) {
    use std::sync::OnceLock;
    static START: OnceLock<Vec<u8>> = OnceLock::new();
    static STOP: OnceLock<Vec<u8>> = OnceLock::new();
    let wave = match cue {
        Cue::Started => START.get_or_init(|| wave(Cue::Started)),
        Cue::Stopped => STOP.get_or_init(|| wave(Cue::Stopped)),
    };
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
    }
    // SND_ASYNC | SND_NODEFAULT | SND_MEMORY. Failure is intentionally silent;
    // visual recording state remains authoritative when audio output is absent.
    unsafe {
        PlaySoundW(
            wave.as_ptr().cast(),
            std::ptr::null_mut(),
            0x0001 | 0x0002 | 0x0004,
        );
    }
}

#[cfg(any(not(windows), test))]
pub fn play(_: Cue) {}

#[cfg(any(windows, test))]
fn wave(cue: Cue) -> Vec<u8> {
    const RATE: u32 = 22050;
    const SAMPLES: u32 = RATE / 8;
    let mut out = Vec::with_capacity(44 + SAMPLES as usize * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + SAMPLES * 2).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(SAMPLES * 2).to_le_bytes());
    let mut phase = 0.0_f32;
    for i in 0..SAMPLES {
        let p = i as f32 / (SAMPLES - 1) as f32;
        let frequency = match cue {
            Cue::Started => 660.0 + 220.0 * p,
            Cue::Stopped => 660.0 - 220.0 * p,
        };
        phase += std::f32::consts::TAU * frequency / RATE as f32;
        let envelope = (std::f32::consts::PI * p).sin().powi(2);
        let sample = (phase.sin() * envelope * 0.08 * i16::MAX as f32) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cues_are_distinct_bounded_pcm_with_soft_edges() {
        let start = wave(Cue::Started);
        let stop = wave(Cue::Stopped);
        assert_ne!(start, stop);
        assert_eq!(&start[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(start[40..44].try_into().unwrap()) as usize,
            start.len() - 44
        );
        for bytes in [start, stop] {
            let samples: Vec<_> = bytes[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes([b[0], b[1]]))
                .collect();
            assert_eq!(samples[0], 0);
            assert_eq!(*samples.last().unwrap(), 0);
            assert!(samples.iter().all(|s| s.unsigned_abs() <= 2622));
        }
    }
}
