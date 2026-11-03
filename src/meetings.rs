//! Meeting detection for Microsoft Teams, Zoom and Google Meet.
//!
//! A meeting is inferred from two local signals: the application's process is
//! running, and that process owns an active Windows audio capture session
//! (`IAudioSessionManager2`). Google Meet runs inside a browser, so its signal
//! is a browser process with an active capture session and is labelled
//! best-effort. The detector debounces both edges so brief glitches never start
//! or finish a transcript. It only reports events; starting capture goes through
//! the same application path as Discord's automatic transcripts.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Shown whenever meeting capture is offered or started.
pub const CONSENT_REMINDER: &str = "Recording a meeting may require the consent of everyone taking part. Tell the other participants that a transcript is being made and check the rules that apply where you and they are.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum App {
    Teams,
    Zoom,
    Meet,
}

impl App {
    pub const ALL: [App; 3] = [App::Teams, App::Zoom, App::Meet];

    pub fn id(self) -> &'static str {
        match self {
            App::Teams => "teams",
            App::Zoom => "zoom",
            App::Meet => "meet",
        }
    }
    #[allow(dead_code, reason = "Retained for settings wiring")]
    pub fn parse_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|app| app.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            App::Teams => "Microsoft Teams",
            App::Zoom => "Zoom",
            App::Meet => "Google Meet",
        }
    }
    /// Meet is recognised only through a browser holding the microphone, which
    /// any other web call would also do.
    pub fn best_effort(self) -> bool {
        matches!(self, App::Meet)
    }
    /// Lower-case executable names whose processes count for this app.
    pub fn process_names(self) -> &'static [&'static str] {
        match self {
            App::Teams => &["ms-teams.exe", "teams.exe", "msteams.exe"],
            App::Zoom => &["zoom.exe"],
            App::Meet => &[
                "chrome.exe",
                "msedge.exe",
                "firefox.exe",
                "brave.exe",
                "vivaldi.exe",
                "opera.exe",
                "chromium.exe",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    /// Show a prompt with the consent reminder and wait for the user.
    #[default]
    #[serde(other)]
    Ask,
    /// Start a transcript as soon as a meeting is confirmed.
    Automatic,
}

/// Persisted preferences. Everything is off by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub teams: bool,
    pub zoom: bool,
    pub meet: bool,
    pub start: StartMode,
}

impl Settings {
    pub fn enabled(&self, app: App) -> bool {
        match app {
            App::Teams => self.teams,
            App::Zoom => self.zoom,
            App::Meet => self.meet,
        }
    }
    pub fn any(&self) -> bool {
        App::ALL.into_iter().any(|app| self.enabled(app))
    }
}

/// One observation of the system for one app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub app: App,
    /// A matching process exists.
    pub running: bool,
    /// A matching process owns an active audio capture session.
    pub capturing: bool,
}

impl Evidence {
    pub fn in_meeting(&self) -> bool {
        self.running && self.capturing
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started { app: App, best_effort: bool },
    Ended { app: App },
}

impl Event {
    pub fn app(&self) -> App {
        match self {
            Event::Started { app, .. } | Event::Ended { app } => *app,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Started { .. } => "started",
            Event::Ended { .. } => "ended",
        }
    }
}

struct Track {
    app: App,
    in_meeting: bool,
    /// Start of the current run of contradicting evidence.
    since: Option<Instant>,
}

/// Debounced per-app meeting state machine driven by synthetic or probed evidence.
pub struct Detector {
    tracks: Vec<Track>,
    start_after: Duration,
    end_after: Duration,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new(Duration::from_secs(3), Duration::from_secs(5))
    }
}

impl Detector {
    /// `start_after` of continuous evidence confirms a meeting; `end_after`
    /// without it confirms the end.
    pub fn new(start_after: Duration, end_after: Duration) -> Self {
        Self {
            tracks: App::ALL
                .into_iter()
                .map(|app| Track {
                    app,
                    in_meeting: false,
                    since: None,
                })
                .collect(),
            start_after,
            end_after,
        }
    }

    pub fn active(&self) -> Vec<App> {
        self.tracks
            .iter()
            .filter(|track| track.in_meeting)
            .map(|track| track.app)
            .collect()
    }

    /// Feeds one probe. Apps missing from `evidence` count as absent. An app
    /// disabled in settings ends immediately and can never start.
    pub fn tick(&mut self, now: Instant, evidence: &[Evidence], settings: &Settings) -> Vec<Event> {
        let mut events = Vec::new();
        for track in &mut self.tracks {
            let enabled = settings.enabled(track.app);
            let present = evidence
                .iter()
                .any(|item| item.app == track.app && item.in_meeting());
            if track.in_meeting {
                if !enabled {
                    track.in_meeting = false;
                    track.since = None;
                    events.push(Event::Ended { app: track.app });
                } else if present {
                    track.since = None;
                } else {
                    let since = *track.since.get_or_insert(now);
                    if now.saturating_duration_since(since) >= self.end_after {
                        track.in_meeting = false;
                        track.since = None;
                        events.push(Event::Ended { app: track.app });
                    }
                }
            } else if enabled && present {
                let since = *track.since.get_or_insert(now);
                if now.saturating_duration_since(since) >= self.start_after {
                    track.in_meeting = true;
                    track.since = None;
                    events.push(Event::Started {
                        app: track.app,
                        best_effort: track.app.best_effort(),
                    });
                }
            } else {
                track.since = None;
            }
        }
        events
    }
}

/// Builds evidence from a process list and the process IDs holding active
/// audio capture sessions. Shared by the Windows probe and the tests.
pub fn evidence(processes: &[(u32, String)], capturing: &[u32]) -> Vec<Evidence> {
    App::ALL
        .into_iter()
        .map(|app| {
            let matching: Vec<u32> = processes
                .iter()
                .filter(|(_, name)| {
                    app.process_names()
                        .contains(&name.to_ascii_lowercase().as_str())
                })
                .map(|(pid, _)| *pid)
                .collect();
            Evidence {
                app,
                running: !matching.is_empty(),
                capturing: matching.iter().any(|pid| capturing.contains(pid)),
            }
        })
        .collect()
}

/// Lists running processes and the IDs of processes with an active audio
/// capture session, then derives evidence for every app.
#[cfg(windows)]
pub fn probe() -> anyhow::Result<Vec<Evidence>> {
    Ok(evidence(
        &windows_probe::processes()?,
        &windows_probe::capturing_processes()?,
    ))
}

#[cfg(not(windows))]
pub fn probe() -> anyhow::Result<Vec<Evidence>> {
    anyhow::bail!("Meeting detection is currently available on Windows.")
}

#[cfg(windows)]
mod windows_probe {
    use anyhow::{Context, Result, ensure};
    use windows::Win32::Media::Audio::{
        AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator, eCapture,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::core::Interface;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    pub fn processes() -> Result<Vec<(u32, String)>> {
        let mut found = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            ensure!(
                !snapshot.is_null() && snapshot as isize != -1,
                "Process list unavailable"
            );
            let mut entry = std::mem::zeroed::<PROCESSENTRY32W>();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more && found.len() < 4096 {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name =
                    String::from_utf16_lossy(&entry.szExeFile[..length]).to_ascii_lowercase();
                found.push((entry.th32ProcessID, name));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        Ok(found)
    }

    /// Process IDs that currently own an active capture (microphone) session on
    /// any active capture endpoint.
    pub fn capturing_processes() -> Result<Vec<u32>> {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
            let result = (|| -> Result<Vec<u32>> {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                        .context("Audio device enumeration unavailable")?;
                let devices = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
                let mut pids = Vec::new();
                for index in 0..devices.GetCount()?.min(16) {
                    let device = devices.Item(index)?;
                    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                    let sessions = manager.GetSessionEnumerator()?;
                    for session in 0..sessions.GetCount()?.min(256) {
                        let control = sessions.GetSession(session)?;
                        let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                            continue;
                        };
                        if control.GetState()? != AudioSessionStateActive {
                            continue;
                        }
                        let pid = control.GetProcessId()?;
                        if pid != 0 && !pids.contains(&pid) {
                            pids.push(pid);
                        }
                    }
                }
                Ok(pids)
            })();
            if initialized.is_ok() {
                CoUninitialize();
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn teams(capturing: bool) -> Vec<Evidence> {
        vec![Evidence {
            app: App::Teams,
            running: true,
            capturing,
        }]
    }

    fn settings(teams: bool, zoom: bool, meet: bool) -> Settings {
        Settings {
            teams,
            zoom,
            meet,
            start: StartMode::Ask,
        }
    }

    #[test]
    fn meetings_start_after_continuous_evidence_and_end_after_a_grace_period() {
        let mut detector = Detector::new(Duration::from_secs(3), Duration::from_secs(5));
        let at = Instant::now();
        let on = settings(true, false, false);
        assert!(detector.tick(at, &teams(true), &on).is_empty());
        // A blip resets the start debounce.
        assert!(
            detector
                .tick(at + Duration::from_secs(2), &teams(false), &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(3), &teams(true), &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(5), &teams(true), &on)
                .is_empty()
        );
        assert_eq!(
            detector.tick(at + Duration::from_secs(6), &teams(true), &on),
            vec![Event::Started {
                app: App::Teams,
                best_effort: false
            }]
        );
        assert_eq!(detector.active(), vec![App::Teams]);
        // Losing the microphone briefly does not end the meeting.
        assert!(
            detector
                .tick(at + Duration::from_secs(7), &teams(false), &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(10), &teams(true), &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(11), &[], &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(15), &[], &on)
                .is_empty()
        );
        assert_eq!(
            detector.tick(at + Duration::from_secs(16), &[], &on),
            vec![Event::Ended { app: App::Teams }]
        );
        assert!(detector.active().is_empty());
        // Starting again requires the full debounce once more.
        assert!(
            detector
                .tick(at + Duration::from_secs(17), &teams(true), &on)
                .is_empty()
        );
        assert!(
            detector
                .tick(at + Duration::from_secs(19), &teams(true), &on)
                .is_empty()
        );
        assert_eq!(
            detector
                .tick(at + Duration::from_secs(20), &teams(true), &on)
                .len(),
            1
        );
    }

    #[test]
    fn disabled_apps_never_start_and_end_at_once_when_switched_off() {
        let mut detector = Detector::new(Duration::ZERO, Duration::from_secs(5));
        let at = Instant::now();
        assert!(
            detector
                .tick(at, &teams(true), &settings(false, true, true))
                .is_empty()
        );
        let on = settings(true, false, false);
        assert_eq!(
            detector
                .tick(at + Duration::from_secs(1), &teams(true), &on)
                .len(),
            1
        );
        assert_eq!(
            detector.tick(
                at + Duration::from_secs(2),
                &teams(true),
                &settings(false, false, false)
            ),
            vec![Event::Ended { app: App::Teams }]
        );
        // A running process without an audio session is not a meeting.
        assert!(
            detector
                .tick(at + Duration::from_secs(3), &teams(false), &on)
                .is_empty()
        );
    }

    #[test]
    fn apps_are_tracked_independently_and_meet_is_best_effort() {
        let mut detector = Detector::new(Duration::ZERO, Duration::ZERO);
        let at = Instant::now();
        let all = settings(true, true, true);
        let evidence = [
            Evidence {
                app: App::Zoom,
                running: true,
                capturing: true,
            },
            Evidence {
                app: App::Meet,
                running: true,
                capturing: true,
            },
        ];
        let events = detector.tick(at, &evidence, &all);
        assert_eq!(
            events,
            vec![
                Event::Started {
                    app: App::Zoom,
                    best_effort: false
                },
                Event::Started {
                    app: App::Meet,
                    best_effort: true
                }
            ]
        );
        assert_eq!(events[1].kind(), "started");
        assert_eq!(events[1].app().label(), "Google Meet");
        assert_eq!(
            detector.tick(at + Duration::from_secs(1), &evidence[..1], &all),
            vec![Event::Ended { app: App::Meet }]
        );
        assert_eq!(detector.active(), vec![App::Zoom]);
    }

    #[test]
    fn evidence_matches_process_names_case_insensitively_against_capture_sessions() {
        let processes = vec![
            (10, "MS-Teams.exe".to_owned()),
            (20, "Zoom.exe".to_owned()),
            (30, "chrome.exe".to_owned()),
            (31, "chrome.exe".to_owned()),
            (40, "Discord.exe".to_owned()),
        ];
        let found = evidence(&processes, &[31, 40]);
        assert_eq!(
            found,
            vec![
                Evidence {
                    app: App::Teams,
                    running: true,
                    capturing: false
                },
                Evidence {
                    app: App::Zoom,
                    running: true,
                    capturing: false
                },
                Evidence {
                    app: App::Meet,
                    running: true,
                    capturing: true
                },
            ]
        );
        assert!(!evidence(&[], &[31])[2].running);
        assert!(App::parse_id("zoom").is_some_and(|app| app == App::Zoom));
        assert!(App::parse_id("skype").is_none());
    }

    #[test]
    fn settings_default_to_everything_off_and_ask_first() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, Settings::default());
        assert!(!settings.any());
        assert_eq!(settings.start, StartMode::Ask);
        let parsed: Settings =
            serde_json::from_str(r#"{"zoom":true,"start":"automatic"}"#).unwrap();
        assert!(parsed.any() && parsed.enabled(App::Zoom) && !parsed.enabled(App::Teams));
        assert_eq!(parsed.start, StartMode::Automatic);
        assert!(!CONSENT_REMINDER.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_probe_lists_this_process_and_tolerates_missing_audio_devices() {
        let processes = windows_probe::processes().unwrap();
        assert!(processes.iter().any(|(pid, _)| *pid == std::process::id()));
        // Without a capture device the session list is simply empty; either way
        // the probe must not fail or report this test process as capturing.
        if let Ok(capturing) = windows_probe::capturing_processes() {
            assert!(!capturing.contains(&std::process::id()));
        }
        assert_eq!(probe().unwrap().len(), App::ALL.len());
    }
}
