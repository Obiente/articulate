use anyhow::{Result, bail};
use std::path::Path;
use transcribe_cpp::{
    Backend, CancelToken, Model, ModelOptions, RunOptions, Session, SessionOptions,
};

pub struct Engine {
    session: Session,
    pub backend: String,
    max_samples: usize,
}

impl Engine {
    pub fn set_cancel_token(&mut self, token: &CancelToken) {
        self.session.set_cancel_token(token);
    }
    pub fn load(path: &Path, cpu: bool, cancel: &CancelToken) -> Result<Self> {
        let model = Model::load_with(
            path,
            &ModelOptions {
                backend: if cpu { Backend::Cpu } else { Backend::Auto },
                device: None,
            },
        )?;
        let caps = model.capabilities();
        anyhow::ensure!(
            caps.native_sample_rate == 16000,
            "This model needs a different audio sample rate"
        );
        let backend = model.backend();
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).clamp(2, 8))
            .unwrap_or(4);
        let mut session = model.session_with(&SessionOptions {
            n_threads: threads as i32,
            ..Default::default()
        })?;
        session.set_cancel_token(cancel);
        if !backend.to_ascii_lowercase().contains("cpu") {
            // Prepare GPU kernels before reporting readiness. The synthetic
            // silence is never presented as a transcript or sent for insertion.
            session.run(&[0.0; 16000], &RunOptions::default())?;
        }
        let max_samples = if caps.max_audio_ms > 0 {
            caps.max_audio_ms as usize * 16
        } else {
            usize::MAX
        };
        Ok(Self {
            session,
            backend,
            max_samples,
        })
    }

    pub fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        let window = (crate::audio::WINDOW_SECONDS * 16000).min(self.max_samples);
        if pcm.len() > window {
            let mut pieces = Vec::new();
            let mut at = 0;
            while at < pcm.len() {
                let left = &pcm[at..];
                let count = if left.len() > window {
                    crate::audio::quiet_boundary(left, 16000, window)
                } else {
                    left.len()
                };
                let text = self.transcribe(&left[..count])?;
                if !text.is_empty() {
                    pieces.push(text);
                }
                at += count;
            }
            return Ok(pieces.join(" "));
        }
        if pcm.len() > self.max_samples {
            bail!(
                "Recording exceeds this model's {:.0}-second limit. Please use a shorter recording.",
                self.max_samples as f64 / 16000.0
            );
        }
        anyhow::ensure!(
            pcm.iter().all(|x| x.is_finite()),
            "Audio contains invalid samples"
        );
        // Reject digital silence only. A volume gate can discard quiet speakers.
        if pcm.is_empty() || pcm.iter().all(|x| x.abs() < 0.00001) {
            return Ok(String::new());
        }
        let result = self.session.run(pcm, &RunOptions::default())?;
        anyhow::ensure!(
            !self.session.was_truncated() && !self.session.was_aborted(),
            "Incomplete transcription. Nothing was inserted."
        );
        Ok(result.text.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires the installed model and TRANSCRIBE_TEST_WAV, runs real CPU inference"]
    fn local_inference_cancellation_and_reuse() {
        transcribe_cpp::disable_logging();
        transcribe_cpp::init_backends_default().unwrap();
        let token = CancelToken::new();
        let mut engine = Engine::load(&crate::model::default_path(), true, &token).unwrap();
        assert!(engine.backend.to_lowercase().contains("cpu"));
        assert_eq!(engine.transcribe(&[0.0; 16000]).unwrap(), "");
        assert!(engine.transcribe(&[f32::NAN; 160]).is_err());
        token.cancel();
        let result = engine.transcribe(&[0.01; 16000]);
        assert!(
            result.is_err(),
            "Cancelled inference must not return completed text"
        );
        token.reset();
        let input = std::env::var_os("TRANSCRIBE_TEST_WAV")
            .expect("Supply a short speech WAV for integration testing");
        let pcm = crate::audio::read_wav(input).unwrap();
        let first = engine.transcribe(&pcm).unwrap();
        assert!(!first.is_empty());
        assert_eq!(
            first,
            engine.transcribe(&pcm).unwrap(),
            "Session state leaked between utterances"
        );
    }
}
