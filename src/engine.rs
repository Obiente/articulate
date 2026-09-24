use anyhow::{Result, bail};
use std::path::Path;
use transcribe_cpp::{
    Backend, CancelToken, Model, ModelOptions, RunOptions, Session, SessionOptions,
};

pub struct Engine {
    session: Session,
    verifier: Option<Session>,
    verifier_options: RunOptions,
    verifier_max_samples: usize,
    pub backend: String,
    max_samples: usize,
    options: RunOptions,
    call_preference: Option<String>,
    pub languages: Vec<String>,
}

impl Engine {
    pub fn set_cancel_token(&mut self, token: &CancelToken) {
        self.session.set_cancel_token(token);
        if let Some(verifier) = &mut self.verifier {
            verifier.set_cancel_token(token);
        }
    }
    pub fn load(path: &Path, cpu: bool, cancel: &CancelToken) -> Result<Self> {
        Self::load_with_language(path, cpu, cancel, None)
    }

    pub fn load_with_language(
        path: &Path,
        cpu: bool,
        cancel: &CancelToken,
        language: Option<String>,
    ) -> Result<Self> {
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
        if let Some(language) = &language {
            anyhow::ensure!(
                caps.languages.contains(language),
                "This speech model does not support the selected language ({language}). Choose Auto-detect or another supported language."
            );
        }
        let options = RunOptions {
            language,
            ..RunOptions::default()
        };
        let backend = model.backend();
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).clamp(2, 8))
            .unwrap_or(4);
        let mut session = model.session_with(&SessionOptions {
            n_threads: threads as i32,
            ..Default::default()
        })?;
        session.set_cancel_token(cancel);
        let mut verifier_options = options.clone();
        let mut verifier_max_samples = 0;
        let verifier_path = crate::model::verifier_path();
        let verifier = if verifier_path.is_file() && verifier_path != path {
            let attempt = (|| -> Result<Session> {
                let verifier_model = Model::load_with(
                    &verifier_path,
                    &ModelOptions {
                        // Keep the primary GPU budget and latency predictable.
                        backend: Backend::Cpu,
                        device: None,
                    },
                )?;
                let verifier_caps = verifier_model.capabilities();
                anyhow::ensure!(
                    verifier_caps.native_sample_rate == 16000,
                    "Verifier needs 16 kHz audio"
                );
                verifier_options.language = verifier_language(
                    verifier_options.language.as_deref(),
                    &verifier_caps.languages,
                );
                verifier_max_samples = if verifier_caps.max_audio_ms > 0 {
                    verifier_caps.max_audio_ms as usize * 16
                } else {
                    usize::MAX
                };
                let mut verifier_session = verifier_model.session_with(&SessionOptions {
                    n_threads: threads as i32,
                    ..Default::default()
                })?;
                verifier_session.set_cancel_token(cancel);
                Ok(verifier_session)
            })();
            match attempt {
                Ok(verifier) => Some(verifier),
                Err(error) => {
                    eprintln!("Second speech model was unavailable: {error}");
                    None
                }
            }
        } else {
            None
        };
        if !backend.to_ascii_lowercase().contains("cpu") {
            // Prepare GPU kernels before reporting readiness. The synthetic
            // silence is never presented as a transcript or sent for insertion.
            session.run(&[0.0; 16000], &options)?;
        }
        let max_samples = if caps.max_audio_ms > 0 {
            caps.max_audio_ms as usize * 16
        } else {
            usize::MAX
        };
        Ok(Self {
            session,
            verifier,
            verifier_options,
            verifier_max_samples,
            backend,
            max_samples,
            options: options.clone(),
            call_preference: None,
            languages: caps.languages,
        })
    }

    /// Calls always detect language per utterance. A preference only breaks
    /// script disagreements between the independent recognizers.
    pub fn with_call_preference<T>(
        &mut self,
        preference: Option<String>,
        run: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let primary_language = self.options.language.take();
        let verifier_language = self.verifier_options.language.take();
        self.call_preference = preference;
        let result = run(self);
        self.options.language = primary_language;
        self.verifier_options.language = verifier_language;
        self.call_preference = None;
        result
    }

    pub fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        self.transcribe_with_verification(pcm, false)
    }

    /// Prefer the faster independent recognizer for provisional dictation.
    /// Final text still comes from the primary model and its second check.
    pub fn transcribe_preview(&mut self, pcm: &[f32]) -> Result<String> {
        if pcm.len() > self.verifier_max_samples || self.verifier.is_none() {
            return self.transcribe(pcm);
        }
        anyhow::ensure!(
            pcm.iter().all(|x| x.is_finite()),
            "Audio contains invalid samples"
        );
        if !crate::speech::contains_voice(pcm) {
            return Ok(String::new());
        }
        let verifier = self.verifier.as_mut().expect("verifier checked above");
        match verifier.run(pcm, &self.verifier_options) {
            Ok(result) if !verifier.was_truncated() && !verifier.was_aborted() => {
                Ok(result.text.trim().to_owned())
            }
            Ok(_) if verifier.was_aborted() => bail!("Transcription cancelled"),
            Err(error) if verifier.was_aborted() => Err(error.into()),
            Ok(_) => self.transcribe(pcm),
            Err(error) => {
                eprintln!("Live speech model was unavailable: {error}");
                self.transcribe(pcm)
            }
        }
    }

    /// The second recognizer checks completed, short utterances only. Live
    /// previews remain responsive and never wait for both models.
    pub fn transcribe_final(&mut self, pcm: &[f32]) -> Result<String> {
        self.transcribe_with_verification(pcm, true)
    }

    fn transcribe_with_verification(&mut self, pcm: &[f32], verify: bool) -> Result<String> {
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
                let text = self.transcribe_with_verification(&left[..count], verify)?;
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
        // Background noise and near silence can elicit invented words from ASR.
        // Require acoustic speech evidence, including quiet voices, first.
        if !crate::speech::contains_voice(pcm) {
            return Ok(String::new());
        }
        let result = self.session.run(pcm, &self.options)?;
        anyhow::ensure!(
            !self.session.was_truncated() && !self.session.was_aborted(),
            "Incomplete transcription. Nothing was inserted."
        );
        let primary = result.text.trim();
        if !verify || primary.is_empty() || pcm.len() > 10 * 16000 {
            return Ok(primary.to_owned());
        }
        let Some(verifier) = self.verifier.as_mut() else {
            return Ok(primary.to_owned());
        };
        let second = match verifier.run(pcm, &self.verifier_options) {
            Ok(second) if !verifier.was_truncated() && !verifier.was_aborted() => second,
            Ok(_) if verifier.was_aborted() => bail!("Transcription cancelled"),
            Ok(_) => return Ok(primary.to_owned()),
            Err(error) if verifier.was_aborted() => return Err(error.into()),
            Err(error) => {
                eprintln!("Second speech check was unavailable: {error}");
                return Ok(primary.to_owned());
            }
        };
        Ok(select_transcript(
            primary,
            second.text.trim(),
            self.call_preference
                .as_deref()
                .or(self.options.language.as_deref()),
            second.language.as_deref(),
        ))
    }
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        ('\u{3040}'..='\u{30ff}').contains(&c)
            || ('\u{3400}'..='\u{9fff}').contains(&c)
            || ('\u{ac00}'..='\u{d7af}').contains(&c)
    })
}

fn contains_devanagari(text: &str) -> bool {
    text.chars().any(|c| ('\u{0900}'..='\u{097f}').contains(&c))
}

fn cjk_language(language: &str) -> bool {
    matches!(
        language.split(['-', '_']).next(),
        Some("zh" | "ja" | "ko" | "yue")
    )
}

fn devanagari_language(language: &str) -> bool {
    matches!(
        language.split(['-', '_']).next(),
        Some("hi" | "mr" | "ne" | "sa")
    )
}

fn verifier_language(hint: Option<&str>, supported: &[String]) -> Option<String> {
    let Some(hint) = hint else {
        // This native release selects the model's auto prompt with no hint.
        return None;
    };
    if supported.iter().any(|lang| lang == hint) {
        return Some(hint.to_owned());
    }
    let prefix = format!("{}-", hint.split(['-', '_']).next().unwrap_or(hint));
    supported
        .iter()
        .find(|lang| lang.starts_with(&prefix))
        .cloned()
}

fn select_transcript(
    primary: &str,
    second: &str,
    hint: Option<&str>,
    detected: Option<&str>,
) -> String {
    // Different recognizers may disagree. Only override when a non-CJK
    // language is established and the primary switched writing systems.
    let non_cjk = detected
        .map(|lang| !cjk_language(lang))
        .or_else(|| hint.map(|lang| !cjk_language(lang)))
        .unwrap_or(false);
    if !second.is_empty() && contains_cjk(primary) && !contains_cjk(second) && non_cjk {
        return second.to_owned();
    }
    // Auto detection stays enabled for every utterance. When the independent
    // recognizer detects a Latin-script language, prefer its Latin spelling
    // over a short primary decode that drifted into Devanagari.
    let non_devanagari = detected
        .map(|lang| !devanagari_language(lang))
        .or_else(|| hint.map(|lang| !devanagari_language(lang)))
        .unwrap_or(false);
    if contains_devanagari(primary)
        && !contains_devanagari(second)
        && second.chars().any(|c| c.is_ascii_alphabetic())
        && non_devanagari
    {
        return second.to_owned();
    }
    primary.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_transcript_verification_respects_language_evidence() {
        assert_eq!(
            select_transcript("你好", "Hello", Some("en"), Some("en")),
            "Hello"
        );
        assert_eq!(
            select_transcript("你好", "Hello", None, Some("en")),
            "Hello"
        );
        assert_eq!(select_transcript("你好", "Hello", None, None), "你好");
        assert_eq!(
            select_transcript("你好", "Hello", Some("zh"), Some("en")),
            "Hello"
        );
        assert_eq!(
            select_transcript("Hello", "", Some("en"), Some("en")),
            "Hello"
        );
        assert_eq!(
            select_transcript("हेलो", "Hello.", None, Some("en-US")),
            "Hello."
        );
        assert_eq!(
            select_transcript("हेलो", "Hello.", None, Some("hi-IN")),
            "हेलो"
        );
        assert_eq!(select_transcript("हेलो", "Hello.", None, None), "हेलो");
        assert_eq!(select_transcript("हेलो", "", Some("en"), None), "हेलो");
        assert_eq!(
            select_transcript("हेलो", "Hello.", Some("en"), Some("hi-IN")),
            "हेलो"
        );
    }

    #[test]
    fn verifier_uses_supported_locale_or_auto() {
        let supported = vec!["en-US".into(), "nl-NL".into(), "zh-CN".into()];
        assert_eq!(
            verifier_language(Some("en"), &supported).as_deref(),
            Some("en-US")
        );
        assert_eq!(
            verifier_language(Some("nl"), &supported).as_deref(),
            Some("nl-NL")
        );
        assert_eq!(verifier_language(Some("fr"), &supported), None);
        assert_eq!(verifier_language(None, &supported), None);
    }

    #[test]
    #[ignore = "Requires TRANSCRIBE_TEST_MODEL and TRANSCRIBE_TEST_WAV for real language-conditioned inference"]
    fn english_hint_reaches_native_model() {
        transcribe_cpp::disable_logging();
        transcribe_cpp::init_backends_default().unwrap();
        let token = CancelToken::new();
        let path = std::env::var_os("TRANSCRIBE_TEST_MODEL").unwrap();
        let mut engine =
            Engine::load_with_language(Path::new(&path), true, &token, Some("en".into())).unwrap();
        assert_eq!(engine.options.language.as_deref(), Some("en"));
        let pcm = crate::audio::read_wav(std::env::var_os("TRANSCRIBE_TEST_WAV").unwrap()).unwrap();
        let text = engine.transcribe(&pcm).unwrap();
        assert!(!text.is_empty(), "Speech fixture must produce words");
        assert!(
            !text.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)
                || ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "English fixture switched writing systems"
        );
        assert_eq!(engine.transcribe(&[0.0; 16000]).unwrap(), "");
        eprintln!(
            "English-conditioned inference produced {} words.",
            text.split_whitespace().count()
        );
    }

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
        let input = std::env::var_os("TRANSCRIBE_TEST_WAV")
            .expect("Supply a short speech WAV for integration testing");
        let pcm = crate::audio::read_wav(input).unwrap();
        let result = engine.transcribe(&pcm);
        assert!(
            result.is_err(),
            "Cancelled inference must not return completed text"
        );
        token.reset();
        let first = engine.transcribe(&pcm).unwrap();
        assert!(!first.is_empty());
        assert_eq!(
            first,
            engine.transcribe(&pcm).unwrap(),
            "Session state leaked between utterances"
        );
    }
}
