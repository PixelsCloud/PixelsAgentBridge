use super::*;
use windows::{
    Win32::Storage::EnhancedStorage::PKEY_Link_TargetParsingPath,
    core::{Interface, PCWSTR, w},
};

fn fail(started: bool, message: impl ToString) -> AppActionError {
    AppActionError::new(started, message)
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn catalog_item(id: &str) -> Result<IShellItem, AppActionError> {
    let folder: IShellItem =
        unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) }
            .map_err(|e| fail(false, e))?;
    let items: IEnumShellItems =
        unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(|e| fail(false, e))?;
    for _ in 0..4096 {
        let mut batch = [None];
        let mut count = 0;
        unsafe { items.Next(&mut batch, Some(&mut count)) }.map_err(|e| fail(false, e))?;
        if count == 0 {
            break;
        }
        if let Some(item) = batch[0].take() {
            if name(&item, SIGDN_DESKTOPABSOLUTEPARSING).ok().as_deref() == Some(id) {
                return Ok(item);
            }
        }
    }
    Err(fail(
        false,
        "application_not_found: identifier is not in this user's application catalog",
    ))
}
fn executable(path: &str) -> Result<String, AppActionError> {
    let p = std::path::Path::new(path);
    if !p.is_absolute()
        || !p.is_file()
        || !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(fail(
            false,
            "application_not_found: expected an absolute executable .exe path",
        ));
    }
    Ok(path.into())
}
fn executable_for_item(item: &IShellItem) -> Option<String> {
    let item: IShellItem2 = item.cast().ok()?;
    let ptr = unsafe { item.GetString(&PKEY_Link_TargetParsingPath) }.ok()?;
    let value = unsafe { ptr.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(ptr.0.cast())) };
    value
        .filter(|v| v.len() <= 4096)
        .and_then(|v| executable(&v).ok())
}

pub(crate) fn act(
    request: &AppActionRequest,
    identity: &UserIdentity,
) -> Result<AppActionResult, AppActionError> {
    if matches!(
        request,
        AppActionRequest::Launch {
            new_instance: true,
            ..
        }
    ) {
        return Err(fail(
            false,
            "unsupported_platform: new_instance is only supported by macOS; no application was launched",
        ));
    }
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }.map_err(|e| fail(false, e))?;
    let _apartment = Apartment;
    let observed = identity
        .observation(
            ExecutionMode::DesktopUser,
            ExecutionEnvironmentSource::InteractiveSession,
        )
        .map_err(|e| fail(false, e))?;
    let (app, file) = match request {
        AppActionRequest::Launch { application, .. } => (Some(application), None),
        AppActionRequest::OpenFile { path, application } => {
            (application.as_ref(), Some(path.as_str()))
        }
    };
    let item = match app {
        Some(AppTarget::Id { id }) => Some(catalog_item(id)?),
        _ => None,
    };
    let mut target = None;
    if let Some(AppTarget::Path { path }) = app {
        target = Some(executable(path)?);
    }
    if file.is_some()
        && let Some(item) = &item
    {
        target = executable_for_item(item);
    }
    let pid = if file.is_some() && item.is_some() && target.is_none() {
        let Some(AppTarget::Id { id }) = app else {
            unreachable!()
        };
        // Packaged apps use their declared file activation contract; never turn
        // an identifier into a raw executable command line.
        let file = wide(file.unwrap());
        let file_item: IShellItem =
            unsafe { SHCreateItemFromParsingName(PCWSTR(file.as_ptr()), None) }
                .map_err(|e| fail(false, e))?;
        let files: IShellItemArray = unsafe { SHCreateShellItemArrayFromShellItem(&file_item) }
            .map_err(|e| fail(false, e))?;
        let manager: IApplicationActivationManager =
            unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER) }
                .map_err(|e| fail(false, e))?;
        let id = wide(id);
        Some(
            unsafe { manager.ActivateForFile(PCWSTR(id.as_ptr()), &files, w!("open")) }
                .map_err(|e| fail(true, format!("application_file_activation_failed: {e}")))?,
        )
    } else {
        let mut info = SHELLEXECUTEINFOW::default();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = w!("open");
        info.nShow = 1;
        let path = wide(
            target
                .as_deref()
                .or(if app.is_none() { file } else { None })
                .unwrap_or(""),
        );
        let parameters = if app.is_some() {
            file.map(|f| {
                // Existing Windows file paths cannot contain quotes or end in a
                // separator; pass exactly one quoted file argument, no shell.
                if f.contains('"') || f.ends_with('\\') {
                    return Err(fail(false, "invalid local file argument"));
                }
                Ok(wide(&format!("\"{f}\"")))
            })
            .transpose()?
        } else {
            None
        };
        info.lpFile = PCWSTR(path.as_ptr());
        if let Some(p) = &parameters {
            info.lpParameters = PCWSTR(p.as_ptr());
        }
        let pidl = if file.is_none()
            && let Some(item) = &item
        {
            let pidl = unsafe { SHGetIDListFromObject(item) }.map_err(|e| fail(false, e))?;
            info.fMask |= SEE_MASK_IDLIST;
            info.lpIDList = pidl.cast();
            info.lpFile = PCWSTR::null();
            Some(pidl)
        } else {
            None
        };
        let result = unsafe { ShellExecuteExW(&mut info) };
        if let Some(pidl) = pidl {
            unsafe { CoTaskMemFree(Some(pidl.cast())) };
        }
        let pid = if info.hProcess.is_invalid() {
            None
        } else {
            let p = unsafe { GetProcessId(info.hProcess) };
            unsafe {
                let _ = CloseHandle(info.hProcess);
            };
            (p != 0).then_some(p)
        };
        result.map_err(|e| fail(true, format!("application_activation_failed: {e}")))?;
        pid
    };
    let instance = pid.and_then(|pid| {
        let marker = pab_os_control::process_identity(pid).ok()?;
        let owner = pab_os_control::execution::process_user_identity(pid).ok()?;
        let mut session = 0;
        if unsafe {
            windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut session)
        } == 0
            || Some(session) != identity.session_id
            || owner.account_id != identity.account_id
            || owner.session_id != identity.session_id
            || pab_os_control::process_identity(pid).ok().as_ref() != Some(&marker)
        {
            return None;
        }
        Some(AppInstance {
            process_id: pid,
            process_identity: marker,
            account_id: Some(owner.account_id),
            session_id: Some(session.to_string()),
        })
    });
    Ok(AppActionResult{request_accepted:true,instance,reused_instance:None,execution_identity:observed,window_ready:None,notes:vec!["Shell acceptance does not prove a new process or a ready window; observe with existing window tools".into()]})
}
