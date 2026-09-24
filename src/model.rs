use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

pub const NAME: &str = "Qwen3-ASR-1.7B-Q8_0.gguf";
pub const SIZE: u64 = 2_185_030_624;
pub const SHA256: &str = "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7";
const URL: &str = "https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf/resolve/3555bd238a8572bbace3ebf60d23b036dc0a5dbe/Qwen3-ASR-1.7B-Q8_0.gguf";
pub const VERIFIER_NAME: &str = "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";
const VERIFIER_SIZE: u64 = 751_094_240;
const VERIFIER_SHA256: &str = "b94545b313b3223fda7b2857a52681da813935c2127643d1e9ff0c23d988089c";
const VERIFIER_URL: &str = "https://huggingface.co/handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/resolve/8139c4ec14bdc45c361adf8d57c27c28e7478272/nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";

pub fn data_dir() -> PathBuf {
    // Controller tests can save preferences. Never let those writes touch a real profile.
    if cfg!(test) {
        if let Some(root) = std::env::var_os("ARTICULATE_BRAIN_TEST_DIR") {
            let root = PathBuf::from(root);
            assert!(
                root.is_absolute(),
                "Use an absolute isolated model test directory"
            );
            return root.join("TranscribeLocal");
        }
        return std::env::temp_dir().join(format!("articulate-tests-{}", std::process::id()));
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TranscribeLocal")
}
pub fn default_path() -> PathBuf {
    data_dir().join("models").join(NAME)
}
pub fn verifier_path() -> PathBuf {
    data_dir().join("models").join(VERIFIER_NAME)
}
pub fn download_verifier(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_file(
        VERIFIER_NAME,
        VERIFIER_SIZE,
        VERIFIER_SHA256,
        VERIFIER_URL,
        &mut progress,
    )
}

// No audio, text, IDs, settings or dictionary data are included in this request.
pub fn download(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_file(NAME, SIZE, SHA256, URL, &mut progress)
}

#[allow(
    dead_code,
    reason = "Preserve speaker-model download for React audio setup"
)]
pub fn download_speakers(mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    download_file(
        crate::speakers::NAME,
        236606560,
        "62faec7b99ad23e323087597604b50728abe85089b6364970b019a845547bf99",
        "https://huggingface.co/handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf/resolve/ae4afbb5c3d33b71cf2dbf600022b655ee706dd0/diar_streaming_sortformer_4spk-v2.1-F16.gguf",
        &mut progress,
    )
}

pub(crate) fn download_file(
    name: &str,
    size: u64,
    expected_hash: &str,
    url: &str,
    progress: &mut impl FnMut(f32),
) -> Result<PathBuf> {
    let path = data_dir().join("models").join(name);
    fs::create_dir_all(path.parent().context("Missing model directory")?)?;
    let part = path.with_extension("gguf.part");
    let result = (|| {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(1800)))
            .build();
        let agent: ureq::Agent = config.into();
        let mut response = agent.get(url).call().context("Model download failed")?;
        let mut reader = response.body_mut().as_reader();
        let mut file = File::create(&part)?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut hash = Sha256::new();
        let mut received = 0u64;
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > size {
                bail!("Downloaded model has an unexpected size");
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])?;
            progress(received as f32 / size as f32);
        }
        if received != size || format!("{:x}", hash.finalize()) != expected_hash {
            bail!("Model integrity check failed. Please retry the download.");
        }
        file.sync_all()?;
        drop(file);
        // Replace only after verification, without first deleting a valid model.
        fs::rename(&part, &path)?;
        Ok(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(part);
    }
    result
}
