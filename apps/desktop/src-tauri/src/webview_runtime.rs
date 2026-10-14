//! Startup fallback for an installed Evergreen runtime whose EdgeUpdate
//! registration is missing. Run before creating threads or a WebView.
use std::path::{Path, PathBuf};

const BROWSER_FOLDER: &str = "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER";

pub fn prepare() {
    // Respect an explicitly configured runtime. Normal discovery remains the
    // first choice so Microsoft's Evergreen update policy continues to apply.
    if std::env::var_os(BROWSER_FOLDER).is_some() || tauri::webview_version().is_ok() {
        return;
    }
    let roots = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join("Microsoft/EdgeWebView/Application"));
    for candidate in candidates(roots) {
        // SAFETY: the entry point calls this before spawning any threads.
        unsafe { std::env::set_var(BROWSER_FOLDER, &candidate) };
        if tauri::webview_version().is_ok() {
            return;
        }
        unsafe { std::env::remove_var(BROWSER_FOLDER) };
    }
}

fn version(path: &Path) -> Option<[u32; 4]> {
    let mut components = path.file_name()?.to_str()?.split('.');
    let mut result = [0; 4];
    for value in &mut result {
        *value = components.next()?.parse().ok()?;
    }
    components.next().is_none().then_some(result)
}

fn candidates(roots: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    for root in roots {
        let Ok(children) = std::fs::read_dir(root) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            if let Some(version) = version(&path) {
                if path.join("msedgewebview2.exe").is_file() {
                    entries.push((version, path));
                }
            }
        }
    }
    entries.sort_by(|left, right| right.0.cmp(&left.0));
    entries.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_numeric_versions_and_ignores_incomplete_or_nonversion_directories() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "9.0.1.1",
            "154.0.4258.62",
            "154.0.4258.48",
            "155.0.0.1",
            "latest",
            "1.2.3.4.5",
        ] {
            let folder = root.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            if name != "155.0.0.1" {
                std::fs::write(folder.join("msedgewebview2.exe"), b"fixture").unwrap();
            }
        }
        let found = candidates([root.path().to_owned(), root.path().join("missing")]);
        assert_eq!(
            found
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["154.0.4258.62", "154.0.4258.48", "9.0.1.1"]
        );
    }
}
