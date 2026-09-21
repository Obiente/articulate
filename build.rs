fn main() {
    tauri_build::build();
    println!("cargo:rerun-if-env-changed=ARTICULATE_NATIVE_AUDIO_DIR");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    embed_assort(&output);
    let native = std::env::var_os("ARTICULATE_NATIVE_AUDIO_DIR").map(std::path::PathBuf::from);
    for (source, name) in [
        (
            "articulate_discord_audio.node",
            "articulate_discord_audio.node",
        ),
        ("licenses/MinHook.txt", "MinHook.txt"),
        ("licenses/Node-API-Headers.txt", "Node-API-Headers.txt"),
    ] {
        let bytes = if let Some(directory) = &native {
            let path = directory.join(source);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes =
                std::fs::read(path).expect("Native audio payload or its license is missing");
            assert!(!bytes.is_empty(), "Native audio payload is empty");
            bytes
        } else {
            Vec::new()
        };
        std::fs::write(output.join(name), bytes).unwrap();
    }
}

fn embed_assort(output: &std::path::Path) {
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeSet, fs, path::PathBuf};

    println!("cargo:rerun-if-env-changed=ARTICULATE_ASSORT_MODELS_DIR");
    let configured = std::env::var_os("ARTICULATE_ASSORT_MODELS_DIR").map(PathBuf::from);
    let directory = configured
        .clone()
        .unwrap_or_else(|| PathBuf::from("assets/assort"));
    let manifest_path = directory.join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let destination = output.join("assort_bundle.rs");
    if configured.is_none() && !manifest_path.exists() {
        fs::write(
            destination,
            "const MANIFEST: &[u8] = &[];\nconst FILES: &[(&str, &[u8])] = &[];\n",
        )
        .unwrap();
        return;
    }
    let manifest = fs::read(&manifest_path).expect("Assort bundle manifest is missing");
    assert!(
        manifest.len() <= 64 * 1024,
        "Assort bundle manifest is too large"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&manifest).expect("Invalid Assort manifest");
    assert_eq!(
        parsed["schema"].as_u64(),
        Some(1),
        "Unsupported Assort manifest"
    );
    let models = parsed["models"]
        .as_array()
        .expect("Assort models are missing");
    assert_eq!(models.len(), 2, "Bundle must contain notes and corrections");
    let mut tasks = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut total = 0_usize;
    let mut source = String::from(
        "const MANIFEST: &[u8] = include_bytes!(\"assort-manifest.json\");\nconst FILES: &[(&str, &[u8])] = &[\n",
    );
    fs::write(output.join("assort-manifest.json"), &manifest).unwrap();
    for model in models {
        let task = model["task"].as_str().expect("Model task missing");
        assert!(
            matches!(task, "notes" | "corrections") && tasks.insert(task),
            "Invalid model task"
        );
        for file in model["files"].as_array().expect("Model files missing") {
            let relative = file["path"].as_str().expect("Model file path missing");
            assert!(
                relative.starts_with(&format!("{task}/"))
                    && relative.split('/').all(|part| !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)))
                    && paths.insert(relative.to_owned()),
                "Unsafe or duplicate Assort path"
            );
            let path = directory.join(relative);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = fs::read(&path).expect("Bundled Assort model file missing");
            total = total
                .checked_add(bytes.len())
                .expect("Assort bundle overflow");
            assert!(
                !bytes.is_empty() && total <= 64 * 1024 * 1024,
                "Invalid Assort bundle size"
            );
            assert_eq!(
                file["sha256"].as_str(),
                Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
                "Assort bundle hash mismatch"
            );
            let embedded = format!("assort-{}.bin", paths.len());
            fs::write(output.join(&embedded), bytes).unwrap();
            source.push_str(&format!(
                "    ({relative:?}, include_bytes!({embedded:?})),\n"
            ));
        }
    }
    source.push_str("];\n");
    fs::write(destination, source).unwrap();
}
