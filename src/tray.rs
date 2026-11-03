use crate::{Action, Bridge};
use std::sync::{Arc, atomic::AtomicBool};
use tauri::{
    Manager,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
pub(crate) struct QuitRequested(pub AtomicBool);
pub(crate) fn setting(app: &tauri::AppHandle, key: &str) -> bool {
    let bridge = app.state::<Arc<Bridge>>();
    bridge.setting_bool(key)
}
pub(crate) fn setup(app: &mut tauri::App) -> tauri::Result<()> {
    app.manage(QuitRequested(AtomicBool::new(false)));
    let show = MenuItem::with_id(app, "show", "Show Articulate", true, None::<&str>)?;
    let start = MenuItem::with_id(
        app,
        "dictation_start",
        "Start dictation",
        true,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(app, "dictation_stop", "Stop dictation", true, None::<&str>)?;
    let call = MenuItem::with_id(app, "call_start", "Start call capture", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &start, &stop, &call, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Articulate");
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| {
        let id = event.id().as_ref();
        if id == "quit" {
            app.state::<QuitRequested>()
                .inner()
                .0
                .store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.close();
            }
        } else if id == "show" {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        } else if let Ok(action) = serde_json::from_value::<Action>(serde_json::json!({"type":id}))
        {
            let app = app.clone();
            let bridge = app.state::<Arc<Bridge>>().inner().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = bridge.action(action) {
                    let window = app.get_webview_window("main");
                    #[cfg(windows)]
                    crate::message_box(
                        window.as_ref(),
                        &error,
                        windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                            | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
                    );
                    #[cfg(not(windows))]
                    let _ = (window, error);
                }
            });
        }
    })
    .build(app)?;
    Ok(())
}
