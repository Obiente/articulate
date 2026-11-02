use anyhow::{Context, Result, ensure};
use std::{path::PathBuf, time::Instant};
use transcribe_cpp::{Backend, Model, ModelOptions, RunOptions};
#[derive(Debug, PartialEq)]
struct Options {
    model: Option<PathBuf>,
    seconds: u32,
    json: Option<PathBuf>,
}
fn parse(args: &[String]) -> Result<Options> {
    let mut result = Options {
        model: None,
        seconds: 5,
        json: None,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().context("Each benchmark option needs a value")?;
        match flag.as_str() {
            "--model" => result.model = Some(value.into()),
            "--seconds" => {
                result.seconds = value.parse().context("Seconds must be a whole number")?;
                ensure!((1..=30).contains(&result.seconds), "Choose 1 to 30 seconds");
            }
            "--json" => result.json = Some(value.into()),
            _ => anyhow::bail!("Unknown benchmark option"),
        }
    }
    Ok(result)
}
#[derive(serde::Serialize)]
struct Measurement {
    backend: &'static str,
    load_ms: Option<f64>,
    inference_ms: Option<f64>,
    real_time_factor: Option<f64>,
    status: &'static str,
}
#[derive(serde::Serialize)]
struct Report {
    schema_version: u32,
    app_version: &'static str,
    audio: &'static str,
    seconds: u32,
    measurements: Vec<Measurement>,
}
pub(super) fn run(args: &[String]) -> Result<()> {
    let options = parse(args)?;
    let path = options.model.unwrap_or_else(crate::model::default_path);
    ensure!(
        path.is_file(),
        "No model is installed. Download one in Settings or pass --model."
    );
    transcribe_cpp::disable_logging();
    transcribe_cpp::init_backends_default()?;
    let _lease = crate::model::acquire_path(&path);
    let pcm = vec![0.0_f32; options.seconds as usize * 16000];
    let mut report = Report {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        audio: "synthetic_silence_16khz_mono",
        seconds: options.seconds,
        measurements: Vec::new(),
    };
    for (name, backend) in [("cpu", Backend::Cpu), ("vulkan", Backend::Vulkan)] {
        let mut measurement = Measurement {
            backend: name,
            load_ms: None,
            inference_ms: None,
            real_time_factor: None,
            status: "unavailable",
        };
        let started = Instant::now();
        if let Ok(model) = Model::load_with(
            &path,
            &ModelOptions {
                backend,
                device: None,
            },
        ) {
            measurement.load_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
            if let Ok(mut session) = model.session() {
                let started = Instant::now();
                if session.run(&pcm, &RunOptions::default()).is_ok() {
                    let seconds = started.elapsed().as_secs_f64();
                    measurement.inference_ms = Some(seconds * 1000.0);
                    measurement.real_time_factor = Some(seconds / options.seconds as f64);
                    measurement.status = "ok";
                } else {
                    measurement.status = "inference_failed";
                }
            } else {
                measurement.status = "session_failed";
            }
        }
        report.measurements.push(measurement);
    }
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = options.json {
        std::fs::write(path, &json)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_arguments() {
        assert_eq!(parse(&[]).unwrap().seconds, 5);
        for args in [
            vec!["--seconds", "0"],
            vec!["--seconds", "31"],
            vec!["--seconds"],
            vec!["--unknown", "x"],
        ] {
            assert!(parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
        assert_eq!(parse(&["--seconds".into(), "2".into()]).unwrap().seconds, 2);
    }
    #[test]
    fn report_has_no_paths_or_transcripts() {
        let report = Report {
            schema_version: 1,
            app_version: "test",
            audio: "synthetic_silence_16khz_mono",
            seconds: 1,
            measurements: vec![Measurement {
                backend: "cpu",
                load_ms: Some(1.0),
                inference_ms: Some(2.0),
                real_time_factor: Some(0.002),
                status: "ok",
            }],
        };
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 5);
        assert_eq!(json["measurements"][0]["real_time_factor"], 0.002);
    }
}
