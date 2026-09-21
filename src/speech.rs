//! Bundled voice detection before ASR. Never classify speech by its output language.
//! State is local to each clip so another participant cannot prime this detector.

const FRAME: usize = 256; // Earshot uses 16 ms frames at 16 kHz.

pub fn contains_voice(pcm: &[f32]) -> bool {
    if pcm.is_empty() {
        return false;
    }
    let peak = pcm.iter().copied().map(f32::abs).fold(0.0, f32::max);
    if !peak.is_finite() || peak < 0.000_01 {
        return false;
    }
    // Give quiet voices useful detector levels, without changing the audio sent
    // to ASR. Gain is bounded; it cannot turn digital silence into a voice.
    let gain = (0.1 / peak).clamp(1.0, 32.0);
    let mut detector = earshot::Detector::default();
    let mut recent = 0_u8;
    let mut frame = [0.0_f32; FRAME];
    // Two trailing frames flush the detector's short analysis context. They are
    // never added to the recording or the recognizer's input.
    for chunk in pcm.chunks(FRAME).chain([&[][..], &[][..]]) {
        frame.fill(0.0);
        for (out, &sample) in frame.iter_mut().zip(chunk) {
            *out = if sample.is_finite() {
                (sample * gain).clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        let voiced = detector.predict_f32(&frame) >= 0.5;
        recent = (recent << 1) | u8::from(voiced);
        // At least 48 ms of evidence in a 128 ms neighborhood allows brief
        // answers, without treating an isolated click as an utterance.
        if recent.count_ones() >= 3 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_low_noise_and_isolated_clicks_are_not_speech() {
        assert!(!contains_voice(&[]));
        assert!(!contains_voice(&vec![0.0; 16000]));
        assert!(!contains_voice(&vec![0.000_04; 16000]));
        let mut seed = 123_u32;
        let noise: Vec<_> = (0..32000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.000_2
            })
            .collect();
        assert!(!contains_voice(&noise));
        let mut click = vec![0.0; 16000];
        click[8000] = 0.7;
        assert!(!contains_voice(&click));
    }

    #[test]
    fn steady_low_hum_and_noise_after_a_loud_clip_are_rejected() {
        let hum: Vec<_> = (0..32000)
            .map(|i| (i as f32 * std::f32::consts::TAU * 60.0 / 16000.0).sin() * 0.000_2)
            .collect();
        assert!(!contains_voice(&hum));
        let _ = contains_voice(&vec![0.2; 16000]);
        assert!(
            !contains_voice(&hum),
            "Each participant/clip has independent detector state"
        );
    }

    #[test]
    #[ignore = "Requires TRANSCRIBE_TEST_WAV with a spoken phrase"]
    fn spoken_fixture_is_detected_at_normal_and_quiet_levels() {
        let path = std::env::var_os("TRANSCRIBE_TEST_WAV").expect("Supply a speech WAV");
        let pcm = crate::audio::read_wav(path).unwrap();
        for gain in [1.0, 0.1, 0.01] {
            let quieter: Vec<_> = pcm.iter().map(|sample| sample * gain).collect();
            let started = std::time::Instant::now();
            assert!(contains_voice(&quieter), "Missed speech at gain {gain}");
            eprintln!(
                "Speech detector: gain {gain}, {} ms for {:.2} s audio",
                started.elapsed().as_millis(),
                pcm.len() as f64 / 16000.0
            );
        }
    }
}
