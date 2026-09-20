#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod audio;
mod audio_cues;
mod call_capture;
mod call_export;
mod call_segments;
mod calls;
mod classification;
mod cleanup;
mod correction_context;
mod dictionary;
mod discord;
mod discord_attribution;
mod engine;
mod export_file;
mod fonts;
mod history;
mod integration;
mod learning;
mod library;
mod live;
mod macros;
mod model;
mod notes;
mod platform;
mod speakers;
mod update;
mod writing_style;

use anyhow::{Context, Result};
use std::{path::PathBuf, time::Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--assort-worker") {
        return classification::worker(&args[1..]);
    }
    if args.as_slice() == ["--verify-assort-models"] {
        classification::verify_builtin_models()?;
        println!("Pretrained Assort notes and correction models are bundled and verified.");
        return Ok(());
    }
    if args.as_slice() == ["--verify-native-audio-payload"] {
        anyhow::ensure!(
            discord::install::native_payload_available(),
            "Native Discord audio payload is missing"
        );
        println!("Native Discord audio payload and notices are bundled.");
        return Ok(());
    }
    if args.iter().any(|a| a == "--discord-check") {
        return discord_check(&args);
    }
    transcribe_cpp::disable_logging();
    transcribe_cpp::init_backends_default()?;
    if args.iter().any(|a| a == "--help") {
        println!(
            "Articulate\n\nNo arguments: open app\n--devices: list inference devices\n--transcribe <audio.wav> [--model <model.gguf>] [--cpu] [--repeat <N>] [--live]\n--call-file <audio.wav> [--cpu]: transcribe remote audio with speaker labels\n--capture-check: check microphone and output capture for three seconds\n--discord-check [--seconds <1-60>]: check local Discord speaker metadata without recording audio\n--download-model: download and verify default model\n\nAudio and transcripts never leave this computer. Model download needs internet."
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--devices") {
        for d in transcribe_cpp::devices() {
            println!("{}: {} ({})", d.kind, d.description, d.name);
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--capture-check") {
        let control = call_capture::Control::new();
        let mut mic = call_capture::Track::open(None, false, control.clone())?;
        let mut output = call_capture::Track::open(None, true, control.clone())?;
        std::thread::sleep(std::time::Duration::from_secs(3));
        control.stop();
        let end = control.end_seconds();
        let a = mic.take_until(end)?;
        let b = output.take_until(end)?;
        println!(
            "{}",
            serde_json::json!({"microphone_samples":a.len(),"output_samples":b.len(),"microphone_peak":a.iter().fold(0.0f32, |p,v|p.max(v.abs())),"output_peak":b.iter().fold(0.0f32, |p,v|p.max(v.abs()))})
        );
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--call-file") {
        let remote = audio::read_wav(args.get(i + 1).context("Supply a remote-audio WAV")?)?;
        let token = transcribe_cpp::CancelToken::new();
        let cpu = args.iter().any(|a| a == "--cpu");
        let mut engine = engine::Engine::load(&model::default_path(), cpu, &token)?;
        let mut tracker = speakers::Tracker::new(cpu)?;
        for (window, pcm) in remote.chunks(8 * 16000).enumerate() {
            let rows = calls::process_window(
                &mut engine,
                &mut tracker,
                &vec![0.0; pcm.len()],
                pcm,
                window as u64 * 8000,
            )?;
            println!("{}", serde_json::to_string(&rows)?);
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--download-model") {
        model::download(|_| {})?;
        println!("Model downloaded and SHA-256 verified.");
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--transcribe") {
        let input = args
            .get(i + 1)
            .context("Supply a WAV file after --transcribe")?;
        let model = args
            .iter()
            .position(|a| a == "--model")
            .map(|i| {
                args.get(i + 1)
                    .map(PathBuf::from)
                    .context("Supply a model path")
            })
            .transpose()?
            .unwrap_or_else(model::default_path);
        let repeat: usize = args
            .iter()
            .position(|a| a == "--repeat")
            .map(|i| {
                args.get(i + 1)
                    .context("Supply a repeat count")?
                    .parse()
                    .context("Invalid count")
            })
            .transpose()?
            .unwrap_or(1);
        anyhow::ensure!((1..=20).contains(&repeat), "Repeat count must be 1 to 20");
        let pcm = audio::read_wav(input)?;
        let started = Instant::now();
        let cancel = transcribe_cpp::CancelToken::new();
        let mut engine = engine::Engine::load(&model, args.iter().any(|a| a == "--cpu"), &cancel)?;
        let load_ms = started.elapsed().as_millis();
        if args.iter().any(|a| a == "--live") {
            let mut end = 19200usize;
            while end < pcm.len() {
                let started = Instant::now();
                let raw = engine.transcribe(&pcm[..end])?;
                let (text, corrections) = cleanup::apply(&raw);
                println!(
                    "{}",
                    serde_json::json!({"kind":"preview", "audio_ms": end / 16, "transcribe_ms":started.elapsed().as_millis(), "text":text, "raw":raw, "corrections":corrections})
                );
                end += 12800;
            }
        }
        for run in 0..repeat {
            let started = Instant::now();
            let text = engine.transcribe(&pcm)?;
            println!(
                "{}",
                serde_json::json!({"text": text, "backend": engine.backend, "audio_ms": pcm.len() * 1000 / 16000, "load_ms": load_ms, "transcribe_ms": started.elapsed().as_millis(), "run": run + 1})
            );
        }
        return Ok(());
    }
    anyhow::ensure!(args.is_empty(), "Unknown arguments. Use --help.");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Articulate")
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!(
                "../assets/brand/app-icon.png"
            ))?)
            .with_inner_size([1200.0, 840.0])
            .with_min_inner_size([850.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Articulate",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("Could not open the app: {e}"))
}

/// Metadata-only local smoke check. Never print participant names or account IDs.
fn discord_check(args: &[String]) -> Result<()> {
    use std::{collections::HashSet, thread, time::Duration};
    let seconds: u64 = args
        .iter()
        .position(|a| a == "--seconds")
        .map(|i| {
            args.get(i + 1)
                .context("Supply seconds after --seconds")?
                .parse()
                .context("Invalid seconds")
        })
        .transpose()?
        .unwrap_or(10);
    anyhow::ensure!((1..=60).contains(&seconds), "Use 1 to 60 seconds");
    let connection = discord::Connection::start();
    let start = Instant::now();
    let mut connected = false;
    let mut peak_participants = 0;
    let mut peak_resolved_names = 0;
    let mut remote_speakers = HashSet::new();
    while start.elapsed() < Duration::from_secs(seconds) {
        let snapshot = connection.snapshot();
        if matches!(snapshot.status, discord::Status::Ready) {
            connected = true;
        }
        if let Some(observation) = snapshot.observation.filter(|o| o.valid) {
            peak_participants = peak_participants.max(observation.participants.len());
            peak_resolved_names = peak_resolved_names.max(
                observation
                    .participants
                    .iter()
                    .filter(|p| !p.name.is_empty())
                    .count(),
            );
            for participant in observation.participants {
                if participant.speaking && !participant.is_self {
                    remote_speakers.insert((observation.generation, participant.id));
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    connection.disconnect();
    println!(
        "{}",
        serde_json::json!({"connected":connected,"duration_seconds":seconds,"peak_participants":peak_participants,"peak_resolved_names":peak_resolved_names,"active_remote_speakers":remote_speakers.len()})
    );
    anyhow::ensure!(
        connected,
        "Discord connection unavailable. Enable its local debugger and retry."
    );
    Ok(())
}
