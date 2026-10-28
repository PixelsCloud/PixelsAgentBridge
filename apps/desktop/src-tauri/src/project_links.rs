#[tauri::command]
pub fn open_project_repository(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("only the main window can open the project repository".into());
    }
    webbrowser::open("https://github.com/PixelsCloud/PixelsAgentBridge")
        .map_err(|_| "could not open the project repository in the default browser".into())
}
