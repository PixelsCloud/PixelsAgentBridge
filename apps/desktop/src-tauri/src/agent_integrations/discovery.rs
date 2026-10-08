use super::{AgentId, find_agent_launcher};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub(super) fn npm_caches(home: Option<&Path>) -> Vec<PathBuf> {
    let mut caches = Vec::new();
    for key in ["npm_config_cache", "NPM_CONFIG_CACHE"] {
        if let Some(value) = env::var_os(key).filter(|p| !p.is_empty()) {
            caches.push(PathBuf::from(value));
        }
    }
    #[cfg(windows)]
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        caches.push(PathBuf::from(local).join("npm-cache"));
    }
    if let Some(home) = home {
        caches.push(home.join(if cfg!(windows) {
            "AppData/Local/npm-cache"
        } else {
            ".npm"
        }));
    }
    caches.sort();
    caches.dedup();
    caches
}

// Detect npm/npx installations without launching npx, downloading packages, or
// interpreting package code. Configuration targets DSH_HOME regardless of launcher.
pub(super) fn find_agent(
    id: AgentId,
    directories: &[PathBuf],
    caches: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(path) = find_agent_launcher(id, directories) {
        return Some(path);
    }
    let package = match id {
        AgentId::Deepseek => "@deepseek-ai/dsh",
        AgentId::Opencode => "opencode-ai",
        _ => return None,
    };
    for directory in directories {
        for root in [
            directory.join("node_modules"),
            directory.join("../lib/node_modules"),
        ] {
            if let Some(entry) = npm_entry(&root.join(package), package, id.executable()) {
                return Some(entry);
            }
        }
    }
    for cache in caches {
        let Ok(entries) = fs::read_dir(cache.join("_npx")) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(entry) = npm_entry(
                &entry.path().join("node_modules").join(package),
                package,
                id.executable(),
            ) {
                return Some(entry);
            }
        }
    }
    None
}

fn npm_entry(root: &Path, package: &str, executable: &str) -> Option<PathBuf> {
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("package.json")).ok()?).ok()?;
    if manifest["name"] != package {
        return None;
    }
    let relative = Path::new(manifest["bin"][executable].as_str()?);
    if relative.is_absolute()
        || relative.components().any(|p| {
            !matches!(
                p,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return None;
    }
    let path = root.join(relative);
    path.is_file().then_some(path)
}
