use std::{env, path::Path};

use pab_protocol::{CpuArchitecture, ExecutionContext, ExecutionScope, OsFamily, PathStyle};
use thiserror::Error;

pub fn detect_native_execution_context() -> Result<ExecutionContext, PlatformDetectionError> {
    let os_family = target_os()?;
    let architecture = target_architecture()?;
    let info = os_info::get();
    let os_name = info.os_type().to_string();
    let os_version = info.version().to_string();
    if os_name.trim().is_empty() || os_version.trim().is_empty() {
        return Err(PlatformDetectionError::MissingOsInformation);
    }
    let cwd = env::current_dir()?;
    let cwd = path_to_string(&cwd)?;
    let path_style = match os_family {
        OsFamily::Windows => PathStyle::Windows,
        OsFamily::Linux | OsFamily::Macos => PathStyle::Posix,
    };
    let environment_revision = environment_revision(
        os_family,
        &os_name,
        &os_version,
        architecture,
        path_style,
        &cwd,
    );
    Ok(ExecutionContext {
        os_family,
        os_name,
        os_version,
        architecture,
        execution_scope: ExecutionScope::Native,
        path_style,
        interpreter: None,
        cwd: Some(cwd),
        environment_revision,
    })
}

fn target_os() -> Result<OsFamily, PlatformDetectionError> {
    if cfg!(target_os = "windows") {
        Ok(OsFamily::Windows)
    } else if cfg!(target_os = "linux") {
        Ok(OsFamily::Linux)
    } else if cfg!(target_os = "macos") {
        Ok(OsFamily::Macos)
    } else {
        Err(PlatformDetectionError::UnsupportedOs(env::consts::OS))
    }
}

fn target_architecture() -> Result<CpuArchitecture, PlatformDetectionError> {
    if cfg!(target_arch = "x86_64") {
        Ok(CpuArchitecture::X86_64)
    } else if cfg!(target_arch = "aarch64") {
        Ok(CpuArchitecture::Aarch64)
    } else {
        Err(PlatformDetectionError::UnsupportedArchitecture(
            env::consts::ARCH,
        ))
    }
}

fn path_to_string(path: &Path) -> Result<String, PlatformDetectionError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or(PlatformDetectionError::NonUnicodeWorkingDirectory)
}

fn environment_revision(
    os_family: OsFamily,
    os_name: &str,
    os_version: &str,
    architecture: CpuArchitecture,
    path_style: PathStyle,
    cwd: &str,
) -> String {
    let mut input = Vec::new();
    for value in [
        "pab-native-environment-v1",
        &format!("{os_family:?}"),
        os_name,
        os_version,
        &format!("{architecture:?}"),
        &format!("{path_style:?}"),
        cwd,
    ] {
        input.extend_from_slice(&(value.len() as u64).to_be_bytes());
        input.extend_from_slice(value.as_bytes());
    }
    format!("native-v1:{}", blake3::hash(&input).to_hex())
}

#[derive(Debug, Error)]
pub enum PlatformDetectionError {
    #[error("operating system {0} is not supported")]
    UnsupportedOs(&'static str),
    #[error("CPU architecture {0} is not supported")]
    UnsupportedArchitecture(&'static str),
    #[error("operating system name or version is unavailable")]
    MissingOsInformation,
    #[error("working directory is not valid Unicode")]
    NonUnicodeWorkingDirectory,
    #[error("working directory cannot be read: {0}")]
    WorkingDirectory(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_context_is_stable_and_valid_for_this_process() {
        let first = detect_native_execution_context().unwrap();
        let second = detect_native_execution_context().unwrap();

        assert_eq!(first, second);
        assert!(first.environment_revision.starts_with("native-v1:"));
        pab_task_runtime::validate_execution_context(&first).unwrap();

        #[cfg(target_os = "windows")]
        assert_eq!(
            (first.os_family, first.path_style),
            (OsFamily::Windows, PathStyle::Windows)
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            (first.os_family, first.path_style),
            (OsFamily::Linux, PathStyle::Posix)
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            (first.os_family, first.path_style),
            (OsFamily::Macos, PathStyle::Posix)
        );

        #[cfg(target_arch = "x86_64")]
        assert_eq!(first.architecture, CpuArchitecture::X86_64);
        #[cfg(target_arch = "aarch64")]
        assert_eq!(first.architecture, CpuArchitecture::Aarch64);
    }
}
