//! Run on an acceptance device, never on the stable control host. All windows
//! stay hidden and use an isolated WebView profile. No product login is loaded.
//! Embed a Common-Controls v6 manifest with the Windows SDK's mt.exe before
//! running: Cargo examples do not inherit Tauri's application resource section.
//! Verifies actual COM environment identity; does not emulate an Evergreen update.
#[cfg(windows)]
#[path = "../src/child_windows.rs"]
mod child_windows;

#[cfg(windows)]
async fn environment_id(window: &tauri::WebviewWindow) -> Result<String, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let _ = sender.send(format!("{:?}", webview.environment()));
        })
        .map_err(|error| error.to_string())?;
    receiver.await.map_err(|error| error.to_string())
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    };
    use tauri::WebviewWindowBuilder;
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("output directory required"),
    );
    std::fs::create_dir_all(&output).unwrap();
    let profile = output.join("profile");
    let exit = Arc::new(AtomicI32::new(1));
    let result_exit = exit.clone();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let application = tauri::Builder::default().setup(move |app| {
        let main = WebviewWindowBuilder::new(app.handle(), "main", tauri::WebviewUrl::External("about:blank".parse().unwrap()))
            .data_directory(profile.clone()).visible(false).build()?;
        child_windows::initialize(app)?;
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            let result = async {
                let main_environment = environment_id(&main).await?;
                let mut children = Vec::new();
                let mut checked = Vec::new();
                for label in ["desktop-viewer-check", "desktop-terminal-check", "file-manager-check", "confirmation-check"] {
                    let profile = profile.clone();
                    let child = child_windows::build(&handle, move |app| {
                        Ok(WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::External("about:blank".parse().unwrap()))
                            .data_directory(profile).visible(false))
                    }).await?;
                    let environment = environment_id(&child).await?;
                    if environment != main_environment {
                        return Err(format!("{label} created an independent environment: {environment}"));
                    }
                    checked.push(serde_json::json!({"label":label,"environment":environment}));
                    children.push(child);
                }
                let report = serde_json::json!({"status":"passed","isolated_profile":true,"visible_windows":false,
                    "main_environment":main_environment,"shared_environment_windows":checked});
                std::fs::write(output.join("result.json"),serde_json::to_vec_pretty(&report).unwrap()).map_err(|error|error.to_string())?;
                println!("{report}");
                for child in children { let _ = child.destroy(); }
                Ok::<_,String>(())
            }.await;
            match result {
                Ok(()) => result_exit.store(0,Ordering::Relaxed),
                Err(error) => {
                    eprintln!("window environment acceptance failed: {error}");
                    let _ = std::fs::write(output.join("failure.txt"),error);
                }
            }
            handle.exit(result_exit.load(Ordering::Relaxed));
        });
        Ok(())
    }).build(context).expect("QA application setup failed");
    application.run_return(|_, _| {});
    std::process::ExitCode::from(exit.load(Ordering::Relaxed) as u8)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("WebView2 environment acceptance requires Windows");
    std::process::exit(1);
}
