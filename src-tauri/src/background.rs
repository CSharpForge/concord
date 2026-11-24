//! Running in the background: a tray icon, hide-to-tray on close, one instance
//! only, and an opt-in "start when I sign in" switch.
//!
//! Concord supports **Windows only**. These behaviours are switched on only
//! there (`BACKGROUND_SUPPORTED`); the code still compiles elsewhere so it can be
//! checked on a development machine. Everything here uses the official Tauri
//! tray API and the official `autostart` / `single-instance` plugins.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Window, WindowEvent};
use tauri_plugin_autostart::ManagerExt;

pub const MAIN_WINDOW: &str = "main";
/// Passed by the OS at login so Concord opens in the tray instead of a window.
pub const START_MINIMIZED_ARG: &str = "--minimized";
const TRAY_ID: &str = "concord-tray";

/// Tray, hide-on-close and start-at-login exist on Windows only.
pub const BACKGROUND_SUPPORTED: bool = cfg!(windows);

/// Set once the tray icon exists. Hiding the window is only safe then: with no tray
/// icon there would be no way to bring a hidden window back.
static TRAY_READY: AtomicBool = AtomicBool::new(false);

/// Did the OS (or the user) ask for a minimized start? `args[0]` is the program.
pub fn should_start_hidden(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == START_MINIMIZED_ARG)
}

/// Start with the window hidden? Only if asked to AND the tray icon exists to reopen it.
pub fn start_hidden(args: &[String], tray_ready: bool) -> bool {
    tray_ready && should_start_hidden(args)
}

/// Hide (instead of quit) when the main window is closed? Only where supported AND
/// the tray icon exists to reopen it.
pub fn should_hide_on_close(supported: bool, tray_ready: bool) -> bool {
    supported && tray_ready
}

/// What a tray menu entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    PauseAll,
    Quit,
}

impl TrayAction {
    pub const OPEN_ID: &'static str = "open";
    pub const PAUSE_ALL_ID: &'static str = "pause_all";
    pub const QUIT_ID: &'static str = "quit";

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            Self::OPEN_ID => Some(Self::Open),
            Self::PAUSE_ALL_ID => Some(Self::PauseAll),
            Self::QUIT_ID => Some(Self::Quit),
            _ => None,
        }
    }
}

/// Bring the main window to the front (from hidden, minimized or behind others).
pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, TrayAction::OPEN_ID, "Open Concord", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, TrayAction::PAUSE_ALL_ID, "Pause all jobs", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TrayAction::QUIT_ID, "Quit Concord", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep1, &pause, &sep2, &quit])?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Concord")
        .menu(&menu)
        // Left click opens the window; right click shows the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match TrayAction::from_id(event.id.as_ref()) {
            Some(TrayAction::Open) => show_main_window(app),
            Some(TrayAction::PauseAll) => {
                tauri::async_runtime::spawn(crate::jobs::pause_all(app.clone()));
            }
            Some(TrayAction::Quit) => app.exit(0),
            None => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Create the tray (Windows) and show the window unless started minimized.
/// The main window is created hidden (see `tauri.conf.json`) to avoid a flash.
///
/// The tray is best effort: if it cannot be created (for example no shell is
/// running), Concord still starts, shows its window, and quits when closed.
pub fn setup(app: &AppHandle, args: &[String]) -> tauri::Result<()> {
    if BACKGROUND_SUPPORTED && setup_tray(app).is_ok() {
        TRAY_READY.store(true, Ordering::Release);
    }
    if !start_hidden(args, TRAY_READY.load(Ordering::Acquire)) {
        show_main_window(app);
    }
    Ok(())
}

/// Closing the main window hides it to the tray (Windows), so jobs keep running.
/// Other windows (the Discord sign-in window) close normally.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        if should_hide_on_close(BACKGROUND_SUPPORTED, TRAY_READY.load(Ordering::Acquire)) {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackgroundStatus {
    /// False off Windows: the UI hides the whole section.
    pub supported: bool,
    /// Is Concord registered to start when the user signs in?
    pub autostart: bool,
}

#[tauri::command]
pub fn background_status(app: AppHandle) -> Result<BackgroundStatus, String> {
    if !BACKGROUND_SUPPORTED {
        return Ok(BackgroundStatus { supported: false, autostart: false });
    }
    let autostart = app.autolaunch().is_enabled().map_err(|e| e.to_string())?;
    Ok(BackgroundStatus { supported: true, autostart })
}

/// Turn start-at-login on or off. Returns the resulting state.
#[tauri::command]
pub fn background_set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    if !BACKGROUND_SUPPORTED {
        return Err("Start at login is only available on Windows.".into());
    }
    let launch = app.autolaunch();
    if enabled {
        launch.enable().map_err(|e| e.to_string())?;
    } else {
        launch.disable().map_err(|e| e.to_string())?;
    }
    launch.is_enabled().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn minimized_start_is_detected_from_arguments_only() {
        assert!(should_start_hidden(&args(&["concord.exe", "--minimized"])));
        assert!(should_start_hidden(&args(&["concord.exe", "--other", "--minimized"])));
        assert!(!should_start_hidden(&args(&["concord.exe"])));
        assert!(!should_start_hidden(&args(&["concord.exe", "--minimised"])));
        // The program name (argument 0) never counts.
        assert!(!should_start_hidden(&args(&["--minimized"])));
        assert!(!should_start_hidden(&[]));
    }

    #[test]
    fn a_missing_tray_icon_never_hides_the_window() {
        // Without a tray icon there would be no way back to a hidden window.
        assert!(!start_hidden(&args(&["concord.exe", START_MINIMIZED_ARG]), false));
        assert!(start_hidden(&args(&["concord.exe", START_MINIMIZED_ARG]), true));
        assert!(!start_hidden(&args(&["concord.exe"]), true), "no --minimized: show the window");
        assert!(!should_hide_on_close(true, false));
        assert!(!should_hide_on_close(false, true));
        assert!(should_hide_on_close(true, true));
        assert!(!should_hide_on_close(false, false));
    }

    #[test]
    fn tray_menu_ids_map_to_actions() {
        assert_eq!(TrayAction::from_id("open"), Some(TrayAction::Open));
        assert_eq!(TrayAction::from_id("pause_all"), Some(TrayAction::PauseAll));
        assert_eq!(TrayAction::from_id("quit"), Some(TrayAction::Quit));
        assert_eq!(TrayAction::from_id(""), None);
        assert_eq!(TrayAction::from_id("Quit"), None);
        assert_eq!(TrayAction::from_id("delete_everything"), None);
    }

    #[test]
    fn autostart_arguments_match_what_the_app_looks_for() {
        // lib.rs registers autostart with START_MINIMIZED_ARG; a login launch must be seen as hidden.
        assert!(should_start_hidden(&args(&["concord.exe", START_MINIMIZED_ARG])));
    }

    #[test]
    fn background_features_are_windows_only() {
        assert_eq!(BACKGROUND_SUPPORTED, cfg!(windows));
    }
}
