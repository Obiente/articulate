//! Optional local sound cues. A bounded worker never blocks live transcription.
use crate::calls::{Row, Update};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MODEL: &str = "sensevoice-small-q8.gguf";
const MODEL_SIZE: u64 = 254_208_320;
const MODEL_HASH: &str = "4ae45c94422de949b387e2e0fb10d7e14e4c42c69db30c3444ecc7d4b844b7c5";
const EXE: &str = "llama-funasr-sensevoice.exe";
const EXE_HASH: &str = "e92b69bc3b0d395dc611572566f91abcf5318ef7a54b27dcdf445bd231ded426";
fn root() -> PathBuf {
    crate::model::data_dir().join("models")
}
pub fn installed() -> bool {
    root()
        .join(MODEL)
        .metadata()
        .is_ok_and(|m| m.len() == MODEL_SIZE)
        && root()
            .join(EXE)
            .metadata()
            .is_ok_and(|m| m.len() == 1_561_600)
}
fn verify(path: &std::path::Path, hash: &str) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    ensure!(
        format!("{:x}", digest.finalize()) == hash,
        "Audio context files failed verification. Reinstall the model."
    );
    Ok(())
}
pub fn install(mut progress: impl FnMut(f32)) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "Audio context currently requires 64-bit Windows."
    );
    let archive = crate::model::download_file(
        "sensevoice-runtime-1.4.16.zip",
        4_967_457,
        "f6a73a548413ba9fbaf2145263ea66ec53cbdad1fb11790dbeeee493e339492e",
        "https://github.com/modelscope/FunASR/releases/download/v1.4.16/funasr-llamacpp-windows-x64.zip",
        &mut |p| progress(p * 0.02),
    )?;
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    let mut exe = zip.by_name(EXE)?;
    ensure!(
        exe.size() == 1_561_600,
        "Unexpected audio context runtime size"
    );
    let part = root().join("sensevoice-runtime.part");
    let mut output = fs::File::create(&part)?;
    std::io::copy(&mut exe, &mut output)?;
    output.sync_all()?;
    drop(output);
    verify(&part, EXE_HASH)?;
    fs::rename(part, root().join(EXE))?;
    crate::model::download_file(
        MODEL,
        MODEL_SIZE,
        MODEL_HASH,
        "https://huggingface.co/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/90c1c61912018b70ada0fcc024ea24aca62f2e63/sensevoice-small-q8.gguf",
        &mut |p| progress(0.02 + p * 0.98),
    )?;
    fs::write(
        root().join("sensevoice-NOTICE.txt"),
        include_str!("../assets/sensevoice/NOTICE.txt"),
    )?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub label: String,
}
// Discard retired tone labels when reading old recordings, including exports.
pub fn deserialize_cues<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Cue>, D::Error> {
    let mut cues = Vec::<Cue>::deserialize(deserializer)?;
    cues.retain(|cue| {
        matches!(
            cue.label.as_str(),
            "Laughing" | "Crying" | "Coughing" | "Sneezing" | "Applause"
        )
    });
    Ok(cues)
}

fn tags(raw: &str, start_ms: u64, end_ms: u64) -> Vec<Cue> {
    let mut rest = raw.trim();
    let mut cues = Vec::new();
    // Only protocol tags at the beginning count. Spoken text cannot inject cues.
    for _ in 0..8 {
        let Some(tail) = rest.strip_prefix("<|") else {
            break;
        };
        let Some((tag, next)) = tail.split_once("|>") else {
            break;
        };
        rest = next;
        let label = match tag {
            "Laughter" => "Laughing",
            "Cry" => "Crying",
            "Cough" => "Coughing",
            "Sneeze" => "Sneezing",
            "Applause" => "Applause",
            _ => continue,
        };
        if !cues.iter().any(|cue: &Cue| cue.label == label) {
            cues.push(Cue {
                start_ms,
                end_ms,
                label: label.into(),
            });
        }
    }
    cues
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn analyze(pcm: &[f32], cancel: &AtomicBool) -> Result<String> {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = crate::model::data_dir().join("audio-context-temp");
    fs::create_dir_all(&dir)?;
    let temp = Temporary(dir.join(format!(
        "{}-{}.wav",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp.0)?;
    let mut wav = hound::WavWriter::new(
        file,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for &sample in pcm {
        wav.write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    wav.finalize()?;
    let mut command = Command::new(root().join(EXE));
    command
        .arg("-m")
        .arg(root().join(MODEL))
        .arg("-a")
        .arg(&temp.0)
        .arg("--keep-tags")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00004000); // Hidden, below normal priority.
    }
    let mut child = command.spawn().context("Could not start audio context")?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Audio context took too long. Transcription continues normally.");
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "Audio context could not read this passage. Transcription continues normally."
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .context("Missing audio context output")?
        .take(65536)
        .read_to_string(&mut output)?;
    Ok(output)
}

pub struct Worker {
    tx: mpsc::SyncSender<(Row, Vec<f32>)>,
    rx: mpsc::Receiver<Result<Row, String>>,
    cancel: Arc<AtomicBool>,
    pending: usize,
    notice: Option<String>,
}
impl Worker {
    pub fn start(enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        let (tx, jobs) = mpsc::sync_channel::<(Row, Vec<f32>)>(8);
        let (results, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("audio-context".into())
            .spawn(move || {
                let ready = verify(&root().join(EXE), EXE_HASH)
                    .and_then(|_| verify(&root().join(MODEL), MODEL_HASH));
                if let Err(error) = ready {
                    let _ = results.send(Err(error.to_string()));
                    return;
                }
                while let Ok((mut row, pcm)) = jobs.recv() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = analyze(&pcm, &stop)
                        .map(|raw| {
                            row.cues = tags(&raw, row.start_ms, row.end_ms);
                            row
                        })
                        .map_err(|e| e.to_string());
                    if results.send(result).is_err() {
                        break;
                    }
                }
            });
        Some(Self {
            tx,
            rx,
            cancel,
            pending: 0,
            notice: spawned.err().map(|e| e.to_string()),
        })
    }
    pub fn submit(&mut self, row: &Row, pcm: &[f32]) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        // Never combine speakers, never feed NaNs, and avoid pure digital silence.
        if pcm.iter().any(|x| !x.is_finite()) || !pcm.iter().any(|x| x.abs() >= 0.000_1) {
            return;
        }
        for (i, chunk) in pcm.chunks(8 * 16000).enumerate() {
            if chunk.len() < 1600 {
                continue;
            }
            let mut row = row.clone();
            row.text.clear();
            row.cues.clear();
            row.start_ms += i as u64 * 8000;
            row.end_ms = row.end_ms.min(row.start_ms + chunk.len() as u64 / 16);
            if row.end_ms <= row.start_ms {
                continue;
            }
            match self.tx.try_send((row, chunk.to_vec())) {
                Ok(()) => self.pending += 1,
                Err(_) => {
                    self.notice = Some(
                        "Some sound cues were skipped to keep transcription responsive.".into(),
                    );
                    break;
                }
            }
        }
    }
    pub fn poll(&mut self, update: &mut impl FnMut(Update)) {
        for result in self.rx.try_iter() {
            self.pending = self.pending.saturating_sub(1);
            match result {
                Ok(row) if !row.cues.is_empty() => update(Update::AudioContext(row)),
                Ok(_) => (),
                Err(error) => {
                    self.pending = 0;
                    self.cancel.store(true, Ordering::Relaxed);
                    self.notice = Some(error);
                }
            }
        }
        if let Some(message) = self.notice.take() {
            update(Update::AudioContextStatus(message));
        }
    }
    pub fn finish(&mut self, update: &mut impl FnMut(Update), abort: &AtomicBool) {
        let start = Instant::now();
        while self.pending > 0
            && start.elapsed() < Duration::from_secs(8)
            && !abort.load(Ordering::Relaxed)
        {
            self.poll(update);
            std::thread::sleep(Duration::from_millis(20));
        }
        self.poll(update);
        if self.pending > 0 {
            update(Update::AudioContextStatus(
                "Some sound cues were skipped. Your transcript is saved.".into(),
            ));
        }
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn apply(rows: &mut Vec<Row>, annotation: &Row) {
    if let Some(row) = rows.iter_mut().find(|row| {
        row.microphone == annotation.microphone
            && row.discord == annotation.discord
            && row.speakers == annotation.speakers
            && row.start_ms < annotation.end_ms
            && row.end_ms > annotation.start_ms
    }) {
        for cue in &annotation.cues {
            if !row.cues.contains(cue) {
                row.cues.push(cue.clone());
            }
        }
    } else {
        crate::calls::append_rows(rows, vec![annotation.clone()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_tone_labels_are_discarded_but_sound_cues_survive() {
        let mut row: crate::calls::Row = serde_json::from_value(serde_json::json!({
            "start_ms": 0, "end_ms": 1000, "microphone": false,
            "speakers": [], "discord": null, "text": "Hello",
            "cues": [
                {"start_ms": 0, "end_ms": 1000, "label": "Possibly happy tone", "tentative": true},
                {"start_ms": 0, "end_ms": 1000, "label": "Laughing", "tentative": false}
            ]
        }))
        .unwrap();
        assert_eq!(row.cues.len(), 1);
        assert_eq!(row.cues[0].label, "Laughing");
        assert_eq!(row.text, "Hello");
        let saved = serde_json::to_string(&row).unwrap();
        assert!(!saved.contains("tone"));
        assert!(!saved.contains("tentative"));
        row.cues.clear();
        let legacy = serde_json::to_value(&row).unwrap();
        assert!(legacy.get("cues").is_none());
        assert!(
            serde_json::from_value::<crate::calls::Row>(legacy)
                .unwrap()
                .cues
                .is_empty()
        );
    }
    #[test]
    fn only_protocol_tags_become_cues() {
        let raw = "<|en|><|HAPPY|><|Laughter|><|withitn|>Hello <|Cry|>";
        let cues = tags(raw, 100, 900);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].label, "Laughing");
        assert!(tags("<|en|><|HAPPY|><|Speech|>Hello", 100, 900).is_empty());
        assert!(tags("<|en|><|NEUTRAL|><|Speech|>Hello", 0, 10).is_empty());
        assert!(tags("Words <|Laughter|>", 0, 10).is_empty());
    }
    #[test]
    #[ignore = "Requires installed SenseVoice and a local WAV"]
    fn real_audio() {
        let path = std::env::var_os("TRANSCRIBE_TEST_WAV").unwrap();
        let pcm = crate::audio::read_wav(path).unwrap();
        let output = analyze(&pcm[..pcm.len().min(8 * 16000)], &AtomicBool::new(false)).unwrap();
        assert!(output.trim().starts_with("<|"));
        if let Ok(expected) = std::env::var("SENSEVOICE_EXPECT_TAG") {
            assert!(output.contains(&expected));
        }
        eprintln!(
            "SenseVoice cues: {:?}",
            tags(&output, 0, pcm.len() as u64 / 16)
        );
        let row = Row {
            start_ms: 0,
            end_ms: pcm.len() as u64 / 16,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: String::new(),
            cues: Vec::new(),
        };
        let mut worker = Worker::start(true).unwrap();
        worker.submit(&row, &pcm);
        let mut annotations = Vec::new();
        worker.finish(
            &mut |update| match update {
                Update::AudioContext(row) => annotations.push(row),
                Update::AudioContextStatus(error) => panic!("{error}"),
                _ => (),
            },
            &AtomicBool::new(false),
        );
        if std::env::var("SENSEVOICE_EXPECT_TAG").is_ok_and(|tag| tag == "<|Laughter|>") {
            assert_eq!(annotations.len(), 1);
            assert_eq!(annotations[0].cues[0].label, "Laughing");
            assert!(
                annotations[0].text.is_empty(),
                "Secondary ASR words must never enter the transcript"
            );
        }
    }
    #[test]
    fn annotations_survive_merging_without_changing_words_or_other_speakers() {
        let row = Row {
            start_ms: 100,
            end_ms: 1200,
            microphone: false,
            speakers: vec![1],
            discord: None,
            text: "Hello.".into(),
            cues: Vec::new(),
        };
        let mut annotation = row.clone();
        annotation.text.clear();
        annotation.start_ms = 0;
        annotation.end_ms = 1300;
        annotation.cues = tags("<|en|><|NEUTRAL|><|Laughter|>", 0, 1300);
        let mut other = row.clone();
        other.speakers = vec![2];
        let mut rows = vec![other, row];
        apply(&mut rows, &annotation);
        apply(&mut rows, &annotation);
        assert!(rows[0].cues.is_empty());
        assert_eq!(rows[1].text, "Hello.");
        assert_eq!(rows[1].cues.len(), 1);
        let saved = serde_json::to_string(&rows).unwrap();
        let restored: Vec<Row> = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored[1].cues, rows[1].cues);
        let old = r#"{"start_ms":0,"end_ms":10,"microphone":true,"speakers":[],"discord":null,"text":"Old recording"}"#;
        assert!(serde_json::from_str::<Row>(old).unwrap().cues.is_empty());
    }
}
