#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod app;
mod audio;
mod audio_cues;
mod brain;
mod call_capture;
#[allow(
    dead_code,
    reason = "Preserve timestamped subtitle and Markdown export formats for React export controls"
)]
mod call_export;
mod call_segments;
mod calls;
#[allow(
    dead_code,
    reason = "Preserve bundled classification workers for React correction review controls"
)]
mod classification;
mod cleanup;
mod cli;
#[allow(
    dead_code,
    reason = "Preserve context-aware correction review for React controls"
)]
mod correction_context;
mod dictionary;
mod discord;
mod discord_attribution;
mod engine;
mod export_file;
mod history;
mod insights;
mod integration;
mod learning;
#[allow(
    dead_code,
    reason = "Preserve vocabulary and shortcut import and export for React controls"
)]
mod library;
mod live;
mod macros;
mod model;
mod notes;
mod platform;
#[allow(
    dead_code,
    reason = "Preserve manual polish and model-management APIs for React controls"
)]
mod polish;
mod sensevoice;
mod speakers;
mod speech;
mod topics;
#[allow(
    dead_code,
    reason = "Preserve verified updater backend for React update controls"
)]
mod update;
mod writing_style;

use app::desktop::{Action, Bridge};
use std::sync::Arc;
use tauri::{Emitter, Manager};

#[tauri::command]
fn desktop_snapshot(
    window: tauri::WebviewWindow,
    bridge: tauri::State<'_, Arc<Bridge>>,
) -> Result<serde_json::Value, String> {
    require_main(&window)?;
    Ok(bridge.snapshot())
}

#[tauri::command]
fn desktop_overlay(bridge: tauri::State<'_, Arc<Bridge>>) -> serde_json::Value {
    bridge.overlay()
}

#[tauri::command]
async fn desktop_action(
    window: tauri::WebviewWindow,
    bridge: tauri::State<'_, Arc<Bridge>>,
    action: Action,
) -> Result<(), String> {
    require_main(&window)?;
    let installing_update = matches!(action, Action::UpdateInstall);
    let bridge = bridge.inner().clone();
    tauri::async_runtime::spawn_blocking(move || bridge.action(action))
        .await
        .map_err(|e| e.to_string())??;
    if installing_update {
        window.app_handle().exit(0);
    }
    Ok(())
}

#[tauri::command]
async fn desktop_close(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    bridge: tauri::State<'_, Arc<Bridge>>,
) -> Result<(), String> {
    require_main(&window)?;
    let bridge = bridge.inner().clone();
    tauri::async_runtime::spawn_blocking(move || bridge.shutdown())
        .await
        .map_err(|e| e.to_string())??;
    app.exit(0);
    Ok(())
}

fn require_main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("This window cannot change application state.".into())
    }
}

fn local_navigation(url: &tauri::Url) -> bool {
    (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(4173))
}

fn setup_overlay(app: &mut tauri::App) -> tauri::Result<()> {
    let overlay = tauri::WebviewWindowBuilder::new(
        app,
        "dictation-overlay",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Articulate dictation")
    .inner_size(420.0, 120.0)
    .transparent(true)
    .decorations(false)
    // Tauri's native shadow gives undecorated Windows windows a 1px border.
    // The transparent overlay must not expose its rectangular window bounds.
    .shadow(false)
    .background_color(tauri::window::Color(0, 0, 0, 0))
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .focusable(false)
    .visible(false)
    .initialization_script("window.__ARTICULATE_OVERLAY__ = true;")
    .on_navigation(local_navigation)
    .build()?;
    overlay.set_ignore_cursor_events(true)?;
    let handle = app.handle().clone();
    let bridge = app.state::<Arc<Bridge>>().inner().clone();
    std::thread::Builder::new()
        .name("dictation-indicator".into())
        .spawn(move || {
            let mut shown = false;
            loop {
                let Some(window) = handle.get_webview_window("dictation-overlay") else {
                    break;
                };
                let visible = bridge.overlay()["visible"].as_bool().unwrap_or(false);
                if visible && !shown {
                    if let Ok(Some(monitor)) = window.primary_monitor() {
                        let area = monitor.work_area();
                        let scale = monitor.scale_factor();
                        let width = (420.0 * scale) as i32;
                        let height = (120.0 * scale) as i32;
                        let position = tauri::PhysicalPosition::new(
                            area.position.x + (area.size.width as i32 - width) / 2,
                            area.position.y + area.size.height as i32
                                - height
                                - (24.0 * scale) as i32,
                        );
                        let _ = window.set_position(position);
                    }
                    if show_indicator(&window, true) {
                        shown = true;
                    }
                } else if !visible && shown && show_indicator(&window, false) {
                    shown = false;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
        .map_err(tauri::Error::Io)?;
    Ok(())
}

fn show_indicator(window: &tauri::WebviewWindow, visible: bool) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SW_HIDE, SW_SHOWNOACTIVATE, ShowWindowAsync,
        };
        window.hwnd().is_ok_and(|handle| unsafe {
            ShowWindowAsync(handle.0, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) != 0
        })
    }
    #[cfg(not(windows))]
    {
        if visible {
            window.show().is_ok()
        } else {
            window.hide().is_ok()
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        #[cfg(windows)]
        if std::env::args_os().len() == 1 {
            let message: Vec<u16> = error.to_string().encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Articulate".encode_utf16().chain(Some(0)).collect();
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    std::ptr::null_mut(),
                    message.as_ptr(),
                    title.as_ptr(),
                    windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                );
            }
        }
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        return cli::run(&args);
    }
    let _instance = Instance::acquire()?;
    #[cfg(windows)]
    tauri::webview_version().map_err(|error| {
        anyhow::anyhow!(
            "Articulate needs the Microsoft Edge WebView2 Runtime. Install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703 and reopen Articulate.\n\n{error}"
        )
    })?;
    tauri::Builder::default()
        .setup(|app| {
            app.manage(Arc::new(Bridge::start().map_err(std::io::Error::other)?));
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .on_navigation(local_navigation)
                .build()?;
            setup_overlay(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_snapshot,
            desktop_action,
            desktop_close,
            desktop_overlay
        ])
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                let bridge = app.state::<Arc<Bridge>>().inner().clone();
                if bridge.has_frontend() {
                    let _ = window.emit("desktop-close-requested", ());
                    return;
                }
                tauri::async_runtime::spawn_blocking(move || {
                    if bridge.shutdown().is_ok() {
                        app.exit(0);
                    }
                });
            }
        })
        .run(tauri::generate_context!())?;
    Ok(())
}

#[cfg(windows)]
struct Instance(Vec<windows_sys::Win32::Foundation::HANDLE>);
#[cfg(windows)]
impl Instance {
    fn acquire() -> anyhow::Result<Self> {
        use windows_sys::Win32::{Foundation::*, System::Threading::CreateMutexW};
        let mut instance = Self(Vec::new());
        // Hold both names so the previous Tauri build also sees this instance.
        for (name, message) in [
            (
                "Local\\Obiente.Articulate",
                "Articulate is already running.",
            ),
            (
                "Local\\Obiente.Articulate.Desktop",
                "An earlier Articulate app is still running. Close its window before opening this build. Your saved documents will remain available.",
            ),
        ] {
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                anyhow::ensure!(
                    !handle.is_null(),
                    "Could not reserve the application instance."
                );
                let already_exists = GetLastError() == ERROR_ALREADY_EXISTS;
                instance.0.push(handle);
                anyhow::ensure!(!already_exists, "{message}");
            }
        }
        anyhow::ensure!(
            !legacy_window_running()?,
            "An earlier Articulate app is still running. Close its window before opening this build. Your saved documents will remain available."
        );
        Ok(instance)
    }
}

#[cfg(any(windows, test))]
fn is_legacy_app_window(title: &str, executable: &str, own_process: bool) -> bool {
    !own_process
        && title == "Articulate"
        && executable
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("articulate.exe"))
}

#[cfg(windows)]
fn legacy_window_running() -> anyhow::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        System::Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        },
    };
    unsafe extern "system" fn inspect(window: HWND, context: LPARAM) -> windows_sys::core::BOOL {
        // Only visible app windows identify the old GUI. Classifier workers and
        // CLI jobs share its executable name but must not block startup.
        unsafe {
            if IsWindowVisible(window) == 0 {
                return 1;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid == 0 || pid == GetCurrentProcessId() {
                return 1;
            }
            let mut title = [0_u16; 256];
            let length = GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32);
            if length <= 0 || String::from_utf16_lossy(&title[..length as usize]) != "Articulate" {
                return 1;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return 1;
            }
            let mut path = vec![0_u16; 32768];
            let mut length = path.len() as u32;
            let available = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if available != 0
                && is_legacy_app_window(
                    "Articulate",
                    &String::from_utf16_lossy(&path[..length as usize]),
                    false,
                )
            {
                *(context as *mut bool) = true;
                return 0;
            }
            1
        }
    }
    let mut found = false;
    let complete = unsafe { EnumWindows(Some(inspect), (&mut found as *mut bool) as LPARAM) };
    anyhow::ensure!(
        complete != 0 || found,
        "Could not check for an earlier Articulate window. Try opening the app again."
    );
    Ok(found)
}

#[cfg(windows)]
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            for handle in &self.0 {
                windows_sys::Win32::Foundation::CloseHandle(*handle);
            }
        }
    }
}
#[cfg(not(windows))]
struct Instance;
#[cfg(not(windows))]
impl Instance {
    fn acquire() -> anyhow::Result<Self> {
        Ok(Self)
    }
}

#[cfg(test)]
mod desktop_shell_tests {
    #[test]
    fn legacy_guard_matches_only_another_articulate_gui() {
        assert!(super::is_legacy_app_window(
            "Articulate",
            r"C:\test-app\ARTICULATE.exe",
            false,
        ));
        for (title, executable, own_process) in [
            ("Articulate", r"C:\test-app\articulate.exe", true),
            ("", r"C:\test-app\articulate.exe", false),
            (
                "Articulate --assort-worker",
                r"C:\test-app\articulate.exe",
                false,
            ),
            ("Articulate", r"C:\test-app\other.exe", false),
            ("Articulate", r"C:\test-app\not-articulate.exe", false),
        ] {
            assert!(!super::is_legacy_app_window(title, executable, own_process));
        }
    }

    #[test]
    fn embedded_navigation_cannot_escape_to_remote_or_file_content() {
        for url in [
            "http://tauri.localhost/index.html",
            "tauri://localhost/index.html",
        ] {
            assert!(super::local_navigation(&url.parse().unwrap()));
        }
        for url in [
            "https://example.com",
            "file:///C:/index.html",
            "http://tauri.localhost.example.com",
            "http://127.0.0.1:9222",
        ] {
            assert!(!super::local_navigation(&url.parse().unwrap()));
        }
    }
}
