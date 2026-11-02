// Synthetic documents for the browser preview. Never real user data.
const now = Date.now();

export const sampleRows = [
  {
    start_ms: 12000,
    end_ms: 32000,
    speaker: "Casey",
    text: "The new onboarding feels much clearer. I think we should keep the first step focused on getting a good microphone level.",
    provisional: false,
  },
  {
    start_ms: 33000,
    end_ms: 55000,
    speaker: "Jordan",
    cues: [{ start_ms: 33000, end_ms: 38000, label: "Laughing" }],
    text: "Agreed. Once someone finishes their first recording, we can show them where their transcript is saved.",
    provisional: false,
  },
  {
    start_ms: 56000,
    end_ms: 84000,
    speaker: "You",
    text: "I will update the checklist and share it before tomorrow. Let us keep the download step short and explain what happens next.",
    provisional: false,
  },
  {
    start_ms: 87000,
    end_ms: 104000,
    speaker: "Casey",
    text: "Sounds good. We will try the revised flow with the team on Friday and use their feedback to decide what to change.",
    provisional: false,
  },
];

const notes =
  "A simpler first recording\n\nKeep onboarding focused on choosing a microphone and making a first recording. Show where the transcript is saved immediately afterward.\n\nNext steps\n\nUpdate and share the onboarding checklist tomorrow. Keep the model download explanation short and clear.\n\nTry the revised flow with the team on Friday, then review the feedback together.";

export const sampleRecords = [
  {
    id: "sample-topic-release",
    title: "Project release",
    kind: "note",
    is_collection: true,
    can_receive_sources: true,
    topic_id: null,
    created_ms: now - 600000,
    updated_ms: now - 600000,
    duration_ms: 0,
    text: "",
    original: "",
    personal_notes:
      "Keep the release focused on a clear first experience.\n\nPersonal reminder: leave time for accessibility testing.",
    rows: [],
  },
  {
    id: "sample-source-checklist",
    title: "Release checklist",
    kind: "note",
    topic_id: "sample-topic-release",
    created_ms: now - 800000,
    updated_ms: now - 800000,
    duration_ms: 18000,
    text: "For the project release, Casey will review the installation checklist on Thursday. Leave time for testing before we publish.",
    original: "",
    personal_notes: "",
    rows: [],
  },
  {
    id: "sample-source-help",
    title: "Help for new users",
    kind: "dictation",
    topic_id: "sample-topic-release",
    created_ms: now - 700000,
    updated_ms: now - 700000,
    duration_ms: 12000,
    text: "One more thought for the release: put the microphone setup and keyboard shortcut on the same page so new users can get started quickly.",
    original: "",
    personal_notes: "",
    rows: [],
  },
  {
    id: "sample-call-1",
    title: "A clearer first recording",
    kind: "call",
    created_ms: now - 3600000,
    updated_ms: now - 3600000,
    duration_ms: 384000,
    text: sampleRows.map((r) => r.text).join("\n\n"),
    original: "",
    personal_notes: notes,
    rows: sampleRows,
  },
  {
    id: "sample-call-2",
    title: "Planning the next release",
    kind: "call",
    created_ms: now - 86400000,
    updated_ms: now - 86400000,
    duration_ms: 1680000,
    text: "The team agreed to finish the setup flow before the next release. Casey will test the Windows installer and Jordan will review the documentation.",
    original: "",
    personal_notes:
      "Next release\n\nFinish the setup flow before release. Casey will test the Windows installer. Jordan will review the documentation.",
    rows: [],
  },
  {
    id: "sample-note-1",
    title: "Ideas worth coming back to",
    kind: "note",
    created_ms: now - 90000000,
    updated_ms: now - 90000000,
    duration_ms: 0,
    text: "",
    original: "",
    personal_notes:
      "Make the small moments feel effortless.\n\nA clear recording cue. A place for every conversation. Notes that are easy to come back to.",
    rows: [],
  },
  {
    id: "sample-dictation-1",
    title: "A quick follow-up",
    kind: "dictation",
    created_ms: now - 7200000,
    updated_ms: now - 7200000,
    duration_ms: 19000,
    text: "Thanks for the conversation earlier. I will send the updated plan tomorrow morning so we can review it before Friday.",
    original: "",
    personal_notes: "",
    rows: [],
  },
];

export const sampleDictionary = [
  {
    heard: "articulate",
    wanted: "Articulate",
    app: null,
    cues: [],
    enabled: true,
  },
  {
    heard: "q wen",
    wanted: "Qwen",
    app: null,
    cues: ["model"],
    enabled: true,
  },
];

export const sampleMacros = [
  {
    trigger: "signature",
    expansion: "Thanks,\nCasey",
    app: null,
    enabled: true,
  },
];

export const sampleSettings = {
  spoken_editing_commands: false,
  input_profiles: [
    { app: "discord.exe", line_break: "ShiftEnter", style: "Chat" },
  ],
  styles: [{ app: "editor.exe", style: "Clean" }],
  recent_dictation_app: "editor.exe",
  clean_speech: true,
  insert: true,
  live_insert: true,
  learn_corrections: true,
  audio_feedback: true,
  microphone: null,
  output: null,
  hotkey: { ctrl: true, alt: true, shift: false, win: false, key: "Space" },
  hotkey_mode: "Hold",
  quick_note_hotkey: {
    ctrl: true,
    alt: true,
    shift: false,
    win: false,
    key: "N",
  },
  discord_auto_connect: true,
  discord_auto_transcribe: false,
  discord_companion: true,
  vencord_auto_update: false,
  automatic_update_checks: true,
  close_to_tray: false,
  start_with_windows: false,
  cpu: false,
};
