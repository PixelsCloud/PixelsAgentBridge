use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

const SHOW: &str = "pixels-show-window";
const QUIT: &str = "pixels-quit";

struct TrayState {
    _icon: TrayIcon,
    show: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    exiting: AtomicBool,
}

pub fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let result = window
            .show()
            .and_then(|_| window.unminimize())
            .and_then(|_| window.set_focus());
        if let Err(error) = result {
            tracing::warn!(%error, "could not restore desktop window from tray");
        }
    }
}

pub fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, SHOW, "Show window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Exit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &separator, &quit])?;
    let icon = app
        .default_window_icon()
        .ok_or_else(|| std::io::Error::other("missing system tray icon"))?
        .clone();
    let tray = TrayIconBuilder::with_id("pixels-desktop")
        .icon(icon)
        .tooltip("Pixels Agent Bridge")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            SHOW => show_window(app),
            QUIT => {
                app.state::<TrayState>()
                    .exiting
                    .store(true, Ordering::SeqCst);
                // Normal Tauri shutdown still runs MCP reporting cleanup.
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayState {
        _icon: tray,
        show,
        quit,
        exiting: AtomicBool::new(false),
    });
    Ok(())
}

pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() == "main"
        && let tauri::WindowEvent::CloseRequested { api, .. } = event
    {
        api.prevent_close();
        if let Err(error) = window.hide() {
            tracing::warn!(%error, "could not hide desktop window to tray");
        }
    }
}

pub fn prevent_exit(app: &tauri::AppHandle, code: Option<i32>) -> bool {
    // Settings-triggered restart must remain possible. Ordinary close/quit actions
    // keep the process alive; the explicit tray action is the user exit route.
    code != Some(tauri::RESTART_EXIT_CODE)
        && !app.state::<TrayState>().exiting.load(Ordering::SeqCst)
}

#[tauri::command]
pub fn set_tray_language(app: tauri::AppHandle, language: &str) -> Result<(), String> {
    let (show, quit) = match language {
        "zh-CN" => ("显示窗口", "退出"),
        "zh-TW" => ("顯示視窗", "結束程式"),
        "en" => ("Show window", "Exit"),
        _ => return Err("Unsupported tray language".into()),
    };
    let state = app.state::<TrayState>();
    state
        .show
        .set_text(show)
        .and_then(|_| state.quit.set_text(quit))
        .map_err(|error| error.to_string())
}
