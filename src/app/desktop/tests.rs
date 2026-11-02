//! Renderer contract and controller action tests.

use super::{Action, actions};
use crate::app::{Command, Settings, tests::app};
use crate::{call_capture, calls, dictionary, history, integration, macros, model};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn action(value: Value) -> Action {
    serde_json::from_value(value).expect("a valid renderer action")
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, found {value}"))
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn row(start_ms: u64, text: &str) -> calls::Row {
    calls::Row {
        cues: Vec::new(),
        start_ms,
        end_ms: start_ms + 1000,
        microphone: true,
        speakers: Vec::new(),
        discord: None,
        text: text.into(),
    }
}

/// A renderer-visible state with every optional part present.
fn populated_app() -> crate::app::App {
    let (mut app, _) = app();
    let mut selected = history::Session::new(history::Kind::Call);
    selected.text = "Saved transcript.".into();
    selected.rows.push(row(0, "Saved row."));
    app.history.items.push(history::Summary::from(&selected));
    app.history.selected = Some(selected);
    let mut dictation = history::Session::new(history::Kind::Dictation);
    dictation.text = "A dictation.".into();
    app.history.dictation = Some(dictation);
    app.history.call = Some(history::Session::new(history::Kind::Call));
    app.call = Some(call_capture::Control::new());
    app.call_rows.push(row(0, "Live row."));
    app
}

#[test]
fn snapshot_sections_have_a_locked_shape() {
    let app = populated_app();
    let sections: BTreeMap<_, _> = app.desktop_sections().into_iter().collect();
    let expected: BTreeMap<&str, BTreeSet<String>> = [
        (
            "shell",
            set(&[
                "ready",
                "loading",
                "busy",
                "status",
                "saving",
                "history_loading",
                "history_error",
                "filing_status",
                "filing_working",
                "source_move_working",
                "summary_working",
            ]),
        ),
        (
            "capture",
            set(&[
                "recording",
                "call_recording",
                "note_recording",
                "seconds",
                "call_status",
                "note_status",
                "call_cues_enabled",
                "call_cues_status",
                "capture_feedback",
            ]),
        ),
        ("dictation", set(&["text", "original", "dictation_session"])),
        ("live", set(&["call_session", "note_session"])),
        ("live_rows", set(&["call_rows", "note_rows"])),
        ("document", set(&["selected"])),
        (
            "notes",
            set(&["notes", "selected_notes", "call_notes", "note_notes"]),
        ),
        ("settings", set(&["settings", "microphones", "outputs"])),
        (
            "models",
            set(&["model_status", "downloading", "managed_models", "cleanup"]),
        ),
        ("discord", set(&["discord"])),
        ("updater", set(&["updater"])),
        ("overlay", set(&["overlay"])),
        ("history", set(&["history"])),
        ("document_body", set(&["selected_body"])),
        ("vocabulary", set(&["dictionary", "macros", "assort"])),
        ("insights", set(&["insights"])),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        sections.keys().copied().collect::<BTreeSet<_>>(),
        expected.keys().copied().collect::<BTreeSet<_>>()
    );
    for (name, keys_expected) in &expected {
        assert_eq!(&keys(&sections[name]), keys_expected, "section {name}");
    }
    let state = app.desktop_snapshot();
    let session_meta = set(&[
        "id",
        "title",
        "kind",
        "personal_notes",
        "audio_packets_lost",
        "created_ms",
        "updated_ms",
        "topic_id",
        "is_collection",
        "can_receive_sources",
        "sources",
    ]);
    assert_eq!(keys(&state["selected"]), session_meta);
    assert_eq!(keys(&state["call_session"]), session_meta);
    assert_eq!(
        keys(&state["selected_body"]),
        set(&["id", "text", "original", "rows"])
    );
    let mut full_session = session_meta.clone();
    full_session.extend(set(&["text", "original", "rows"]));
    assert_eq!(keys(&state["dictation_session"]), full_session);
    let row_keys = set(&[
        "start_ms",
        "end_ms",
        "speaker",
        "speaker_labels",
        "text",
        "cues",
        "provisional",
    ]);
    assert_eq!(keys(&state["call_rows"][0]), row_keys);
    assert_eq!(keys(&state["selected_body"]["rows"][0]), row_keys);
    assert_eq!(
        keys(&state["history"][0]),
        set(&[
            "id",
            "title",
            "kind",
            "created_ms",
            "updated_ms",
            "preview",
            "duration_ms",
            "topic_id",
            "is_collection",
            "can_receive_sources",
        ])
    );
    assert_eq!(
        keys(&state["settings"]),
        set(&[
            "clean_speech",
            "insert",
            "live_insert",
            "cpu",
            "transcription_language",
            "audio_context",
            "learn_corrections",
            "audio_feedback",
            "microphone",
            "output",
            "hotkey",
            "quick_note_hotkey",
            "quick_note_hotkey_pending",
            "quick_note_shortcut_error",
            "hotkey_mode",
            "discord_auto_connect",
            "discord_auto_transcribe",
            "discord_companion",
            "vencord_auto_update",
            "close_to_tray",
            "start_with_windows",
            "automatic_update_checks",
        ])
    );
    assert_eq!(
        keys(&state["model_status"]),
        set(&[
            "speech_installed",
            "backend",
            "model_name",
            "speech_languages",
            "model_custom",
            "speakers_installed",
            "speakers_working",
            "speakers_progress",
            "speakers_status",
            "context_installed",
            "context_progress",
            "context_status",
            "context_supported",
            "verifier_installed",
            "verifier_working",
            "verifier_progress",
            "verifier_status",
            "cleanup_installed",
            "cleanup_working",
            "cleanup_status",
            "cleanup_progress",
        ])
    );
    assert_eq!(
        keys(&state["overlay"]),
        set(&["visible", "recording", "finishing", "message"])
    );
    assert_eq!(state["updater"], json!({"state": "idle"}));
}

#[test]
fn every_state_key_belongs_to_exactly_one_section() {
    let app = populated_app();
    let mut owners = BTreeMap::new();
    for (section, value) in app.desktop_sections() {
        for key in keys(&value) {
            if let Some(previous) = owners.insert(key.clone(), section) {
                panic!("{key} is in both {previous} and {section}");
            }
        }
    }
    assert!(!owners.contains_key("controller"), "the bridge owns health");
}

#[test]
fn live_rows_are_sent_once_and_not_inside_session_views() {
    let state = populated_app().desktop_snapshot();
    assert_eq!(state["call_rows"][0]["text"], "Live row.");
    for session in ["call_session", "selected"] {
        assert!(state[session].get("rows").is_none(), "{session}");
        assert!(state[session].get("text").is_none(), "{session}");
    }
    assert_eq!(state["selected_body"]["rows"][0]["text"], "Saved row.");
}

#[test]
fn renderer_action_types_match_the_frontend_catalog() {
    let source = include_str!("../../../ui/src/actions.js");
    let catalog = source
        .split_once("export const actionCatalog = Object.freeze({")
        .and_then(|(_, rest)| rest.split_once("});"))
        .map(|(body, _)| body)
        .expect("the action catalog");
    let frontend: BTreeSet<String> = catalog
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once(':')?;
            (rest.trim_start().starts_with('{')
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .then(|| name.to_owned())
        })
        .collect();
    let mut native = BTreeSet::new();
    for (feature, types) in actions::action_types() {
        assert!(!types.is_empty(), "{feature} lists its types");
        for name in types {
            assert!(native.insert(name.clone()), "{name} is declared twice");
        }
    }
    assert_eq!(frontend, native);
}

#[test]
fn lifecycle_actions_require_ready_state_before_external_changes() {
    let (mut app, _) = app();
    assert!(
        app.desktop_action(action(json!({"type": "update_download"})))
            .is_err()
    );
    assert!(
        app.desktop_action(action(json!({"type": "update_install"})))
            .is_err()
    );
    assert!(app.pending_install.is_none());
    app.call = Some(call_capture::Control::new());
    assert!(
        app.desktop_action(action(json!({"type": "discord_relaunch"})))
            .is_err()
    );
    assert!(
        app.desktop_action(action(json!({"type": "companion_install"})))
            .is_err()
    );
    assert!(!app.settings.discord_companion);
    assert!(action(json!({"type": "update_install"})).installs_update());
    assert!(!action(json!({"type": "update_check"})).installs_update());
}

#[test]
fn closing_and_updating_wait_for_downloads() {
    let (mut app, _) = app();
    app.preview_inflight = false;
    app.speakers_downloading = Some(0.4);
    let error = app.desktop_shutdown().unwrap_err();
    assert!(error.contains("speaker identification"), "{error}");
    app.speakers_downloading = None;
    app.downloading = Some(0.1);
    app.context_progress = Some(0.2);
    let error = app.desktop_shutdown().unwrap_err();
    assert!(error.contains("speech model and audio context"), "{error}");
    app.downloading = None;
    app.context_progress = None;
    assert!(app.desktop_active_downloads().is_empty());
}

#[test]
fn invalid_model_selection_does_not_change_settings_or_expose_private_path() {
    let (mut app, _) = app();
    let patch = action(json!({"type":"settings_patch", "cpu":true,"model_path":"relative.gguf"}));
    assert!(app.desktop_action(patch).is_err());
    assert!(!app.settings.cpu);
    app.settings.model_path = std::env::temp_dir()
        .join("synthetic-private-model-folder")
        .join("speech.gguf")
        .to_string_lossy()
        .into_owned();
    let snapshot = app.desktop_snapshot();
    assert_eq!(snapshot["model_status"]["model_name"], "speech.gguf");
    assert!(
        !snapshot
            .to_string()
            .contains("synthetic-private-model-folder")
    );
}

#[test]
fn a_settings_patch_is_validated_completely_before_anything_changes() {
    let (mut app, _) = app();
    // The audio context model is not installed in tests, so this patch fails
    // after several valid fields; none of them may change.
    let patch = action(json!({
        "type": "settings_patch",
        "clean_speech": false,
        "insert": true,
        "hotkey_mode": "Toggle",
        "microphone": "Synthetic microphone",
        "audio_context": true,
    }));
    assert!(app.desktop_action(patch).is_err());
    assert!(app.settings.clean_speech);
    assert!(!app.settings.insert);
    assert_eq!(app.settings.hotkey_mode, crate::platform::HotkeyMode::Hold);
    assert!(app.settings.microphone.is_none());
    // Conflicting shortcuts are rejected before either is registered.
    let shortcut = json!({"ctrl": true, "alt": true, "shift": false, "win": false, "key": "K"});
    let patch = action(json!({
        "type": "settings_patch",
        "clean_speech": false,
        "hotkey": shortcut,
        "quick_note_hotkey": shortcut,
    }));
    assert!(app.desktop_action(patch).is_err());
    assert!(app.settings.clean_speech);
    assert!(!app.hotkey_pending && !app.quick_note_hotkey_pending);
    let patch = action(json!({"type": "settings_patch", "microphone": ""}));
    app.settings.microphone = Some("Previous".into());
    app.desktop_action(patch).unwrap();
    assert!(
        app.settings.microphone.is_none(),
        "empty selects the default"
    );
}

#[test]
fn moves_and_deletions_lock_only_the_documents_they_affect() {
    let (mut app, _) = app();
    let mut topic = history::Session::new(history::Kind::Note);
    topic.personal_notes = "My existing words.".into();
    let patch = |id: &str, notes: &str| {
        action(json!({"type": "session_patch", "id": id, "personal_notes": notes}))
    };
    app.history.selected = Some(topic.clone());
    app.notetaker.moving = true;
    let error = app
        .desktop_action(patch(&topic.id, "A concurrent edit."))
        .unwrap_err();
    assert!(error.contains("moving between topics"), "{error}");
    assert_eq!(
        app.history.selected.as_ref().unwrap().personal_notes,
        "My existing words."
    );
    assert_eq!(app.desktop_snapshot()["source_move_working"], true);
    app.notetaker.moving = false;
    // Deleting an unrelated recording neither blocks nor locks this note.
    app.history
        .pending_deletions
        .insert("an-unrelated-recording".into());
    assert_eq!(app.desktop_snapshot()["source_move_working"], false);
    assert!(app.desktop_edit_blocked(&topic).is_none());
    // Deleting a source of this topic does.
    let source = history::Session::new(history::Kind::Dictation);
    let mut summary = history::Summary::from(&source);
    summary.topic_id = Some(topic.id.clone());
    app.history.items.push(summary);
    app.history.pending_deletions.insert(source.id.clone());
    let error = app
        .desktop_action(patch(&topic.id, "An edit while deleting a source."))
        .unwrap_err();
    assert!(
        error.contains("recording in this topic is being deleted"),
        "{error}"
    );
    assert_eq!(
        app.history.selected.as_ref().unwrap().personal_notes,
        "My existing words."
    );
    assert_eq!(app.desktop_snapshot()["source_move_working"], true);
    app.history.pending_deletions.insert(topic.id.clone());
    assert_eq!(
        app.desktop_edit_blocked(&topic),
        Some("This document is being deleted.")
    );
}

#[test]
fn spoken_note_snapshot_stays_separate_from_calls_and_dictation() {
    let (mut app, _) = app();
    app.history.call = Some(history::Session::new(history::Kind::Note));
    app.call = Some(call_capture::Control::new());
    app.call_rows.push(row(0, "An evolving thought."));
    let state = app.desktop_snapshot();
    assert_eq!(state["note_recording"], true);
    assert_eq!(state["call_recording"], false);
    assert_eq!(state["recording"], false);
    assert_eq!(state["note_rows"][0]["text"], "An evolving thought.");
    assert!(state["call_rows"].as_array().unwrap().is_empty());
    assert!(state["call_session"].is_null());
    assert_eq!(state["note_session"]["kind"], "note");
}

#[test]
fn transcription_language_is_validated_saved_and_sent_to_the_speech_worker() {
    let (mut app, commands) = app();
    let model_path = model::data_dir().join("language-test.gguf");
    std::fs::create_dir_all(model::data_dir()).unwrap();
    std::fs::write(
        &model_path,
        b"synthetic model path; no inference worker in this test",
    )
    .unwrap();
    app.settings.model_path = model_path.to_string_lossy().into_owned();
    let change = |language: &str| {
        action(json!({"type":"settings_patch", "transcription_language":language}))
    };
    app.desktop_action(change("en")).unwrap();
    assert_eq!(app.settings.transcription_language, "en");
    assert!(
        matches!(commands.try_recv().unwrap(), Command::Load(_, _, Some(language)) if language == "en")
    );
    let restored: Settings =
        serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
    assert_eq!(restored.transcription_language, "en");
    app.loading = false;
    assert!(app.desktop_action(change("invalid")).is_err());
    assert_eq!(app.settings.transcription_language, "en");
    app.desktop_action(change("")).unwrap();
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Load(_, _, None)
    ));
    assert!(
        serde_json::from_str::<Settings>("{}")
            .unwrap()
            .transcription_language
            .is_empty()
    );
}

#[test]
fn disabling_learning_stops_field_observation_and_discards_pending_local_edit() {
    let (mut app, _) = app();
    let (sender, events) = mpsc::channel();
    app.integration_tx = sender;
    app.text = "Synthetic corrected text.".into();
    app.edit_baseline = "Earlier text.".into();
    app.edited_at = Some(Instant::now());
    app.desktop_set_learning(false);
    assert!(!app.settings.learn_corrections);
    assert!(matches!(
        events.try_recv().unwrap(),
        integration::Action::StopLearning
    ));
    assert!(app.edited_at.is_none());
    assert_eq!(app.edit_baseline, app.text);
    app.desktop_set_learning(true);
    assert!(app.edited_at.is_none());
    assert!(events.try_recv().is_err());
    // The settings patch applies the same side effects after saving.
    app.edited_at = Some(Instant::now());
    app.desktop_action(action(
        json!({"type": "settings_patch", "learn_corrections": false}),
    ))
    .unwrap();
    assert!(!app.settings.learn_corrections);
    assert!(app.edited_at.is_none());
    assert!(matches!(
        events.try_recv().unwrap(),
        integration::Action::StopLearning
    ));
}

#[test]
fn renderer_snapshot_excludes_pairing_key_and_model_path() {
    let (mut app, _) = app();
    app.settings.discord_pairing_key = "synthetic-private-pairing-value".into();
    app.settings.model_path = "synthetic-private-model-path".into();
    let snapshot = app.desktop_snapshot();
    let serialized = snapshot.to_string();
    assert!(!serialized.contains("synthetic-private"));
    assert!(snapshot["settings"]["clean_speech"].is_boolean());
    assert!(snapshot["history"].is_array());
}

#[test]
fn renderer_cannot_supply_arbitrary_settings_or_actions() {
    assert!(
        serde_json::from_value::<Action>(
            json!({"type":"settings_patch", "discord_pairing_key":"replace"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<Action>(json!({"type":"run_shell", "command":"anything"}))
            .is_err()
    );
    assert!(serde_json::from_value::<Action>(json!({"command":"anything"})).is_err());
    assert!(
        serde_json::from_value::<Action>(json!({"type":"call_start", "source":"elsewhere"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<Action>(json!({"type":"settings_patch", "clean_speech":false}))
            .is_ok()
    );
}

#[test]
fn cancel_dictation_cannot_cancel_a_call() {
    let (mut app, _) = app();
    app.call = Some(call_capture::Control::new());
    let token = app.cancel.clone();
    assert!(
        app.desktop_action(action(json!({"type": "dictation_cancel"})))
            .is_err()
    );
    assert!(!token.is_cancelled());
}

#[test]
fn document_commands_patch_only_notes_and_title_and_persist() {
    let directory = std::env::temp_dir().join(format!(
        "articulate-desktop-contract-{}",
        history::Session::new(history::Kind::Note).id
    ));
    let (mut app, _) = app();
    app.history.worker = Some(history::Worker::test_directory(directory.clone()));
    let mut session = history::Session::new(history::Kind::Dictation);
    session.text = "Synthetic transcript remains unchanged.".into();
    session.original = "Synthetic original remains unchanged.".into();
    let id = session.id.clone();
    app.history.selected = Some(session);
    app.desktop_action(action(json!({
        "type": "session_patch",
        "id": id,
        "title": "Reviewed document",
        "personal_notes": "A deliberately edited note.",
    })))
    .unwrap();
    let mut current = history::Session::new(history::Kind::Call);
    let call_id = current.id.clone();
    current.text = "Earlier source.".into();
    app.history.selected = Some(current.clone());
    current.text = "The latest source includes the completed call.".into();
    app.history.call = Some(current);
    app.desktop_action(action(json!({
        "type": "session_patch",
        "id": call_id,
        "personal_notes": "Keep the latest source.",
    })))
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app
        .history
        .worker
        .as_ref()
        .unwrap()
        .saves_settled()
        .unwrap()
    {
        assert!(Instant::now() < deadline, "Isolated history save timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(app);
    let history = history::History::open(directory.clone()).unwrap();
    let saved = history.load(&id).unwrap();
    assert_eq!(saved.text, "Synthetic transcript remains unchanged.");
    assert_eq!(saved.original, "Synthetic original remains unchanged.");
    assert_eq!(saved.title, "Reviewed document");
    assert!(saved.title_is_manual);
    assert_eq!(saved.personal_notes, "A deliberately edited note.");
    let saved_call = history.load(&call_id).unwrap();
    assert_eq!(
        saved_call.text,
        "The latest source includes the completed call."
    );
    assert_eq!(saved_call.personal_notes, "Keep the latest source.");
    drop(history);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn indicator_visibility_expires_without_focus_or_raw_text() {
    let (mut app, _) = app();
    app.raw = "Unrelated synthetic private transcript".into();
    assert_eq!(app.desktop_snapshot()["overlay"]["visible"], false);
    app.overlay_message = "Ready to copy".into();
    app.overlay_until = Some(Instant::now() + Duration::from_secs(1));
    let snapshot = app.desktop_snapshot();
    assert_eq!(snapshot["overlay"]["visible"], true);
    assert_eq!(snapshot["overlay"]["message"], "Ready to copy");
    assert!(!snapshot["overlay"].to_string().contains(&app.raw));
    app.overlay_until = Some(Instant::now() - Duration::from_secs(1));
    assert_eq!(app.desktop_snapshot()["overlay"]["visible"], false);
}

#[test]
fn stale_editor_cannot_delete_same_phrase_in_a_different_scope() {
    let (mut app, _) = app();
    let original = dictionary::validate("project", "Project").unwrap();
    let mut other = original.clone();
    other.app = Some("editor.exe".into());
    app.settings.entries = vec![other.clone()];
    let result = app.desktop_action(action(json!({
        "type": "dictionary_delete",
        "index": 0,
        "expected_heard": original.heard,
        "expected_entry": serde_json::to_value(&original).unwrap(),
    })));
    assert!(result.is_err());
    assert!(app.settings.entries[0] == other);
    let original = macros::validate("signature", "A synthetic signature", "").unwrap();
    let mut other = original.clone();
    other.app = Some("editor.exe".into());
    app.settings.macros = vec![other];
    let result = app.desktop_action(action(json!({
        "type": "macro_delete",
        "index": 0,
        "expected_trigger": original.trigger,
        "expected_macro": serde_json::to_value(original).unwrap(),
    })));
    assert!(result.is_err());
    assert_eq!(app.settings.macros[0].app.as_deref(), Some("editor.exe"));
}

#[test]
fn model_review_shapes_are_locked() {
    let (app, _) = app();
    let state = app.desktop_snapshot();
    assert_eq!(
        keys(&state["cleanup"]),
        set(&["installed", "working", "status", "progress", "preview"])
    );
    assert_eq!(
        keys(&state["assort"]),
        set(&["working", "status", "candidates", "scores"])
    );
    assert_eq!(
        keys(&state["notes"]),
        set(&[
            "ready",
            "acceleration_installed",
            "working",
            "status",
            "progress",
            "download_status",
            "downloading"
        ])
    );
    assert_eq!(
        keys(&state["managed_models"][0]),
        set(&["id", "installed", "disk_size", "path", "in_use"])
    );
    assert!(
        state["settings"]["automatic_update_checks"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(state["settings"]["close_to_tray"], false);
    assert_eq!(state["settings"]["start_with_windows"], false);
}
#[test]
fn leased_models_cannot_be_deleted_through_renderer() {
    let (mut app, _) = app();
    for id in [
        model::ModelId::Speech,
        model::ModelId::Verifier,
        model::ModelId::Speaker,
        model::ModelId::SoundCues,
        model::ModelId::Polish,
        model::ModelId::Notes,
    ] {
        let _lease = model::acquire(id);
        assert!(
            app.desktop_action(action(json!({"type":"model_delete", "id":id})))
                .is_err()
        );
    }
}

#[test]
fn review_actions_validate_fields_and_cancel_without_changing_text() {
    let (mut app, _) = app();
    app.text = "Synthetic dictation.".into();
    for kind in ["polish_cancel", "assort_cancel"] {
        app.desktop_action(action(json!({"type":kind}))).unwrap();
        assert_eq!(app.text, "Synthetic dictation.");
    }
    assert!(
        serde_json::from_value::<Action>(
            json!({"type":"assort_apply","index":-1,"saved_spelling":true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<Action>(json!({"type":"model_delete","id":"unknown"})).is_err()
    );
    app.desktop_action(action(json!({"type":"settings_patch","automatic_update_checks":false,"close_to_tray":true,"start_with_windows":true}))).unwrap();
    assert!(!app.settings.automatic_update_checks);
    assert!(app.settings.close_to_tray);
    assert!(app.settings.start_with_windows);
}
