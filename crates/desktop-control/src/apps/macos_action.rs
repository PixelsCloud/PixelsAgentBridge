use super::*;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSError, NSString, NSURL};

fn fail(started: bool, message: impl ToString) -> AppActionError {
    AppActionError::new(started, message)
}
fn instance(app: &NSRunningApplication, identity: &UserIdentity) -> Option<AppInstance> {
    let pid = app.processIdentifier();
    if pid <= 0 || app.isTerminated() {
        return None;
    }
    let marker = pab_os_control::process_identity(pid as u32).ok()?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    } != size
        || format!("uid:{}", info.pbi_uid) != identity.account_id
        || pab_os_control::process_identity(pid as u32).ok().as_ref() != Some(&marker)
    {
        return None;
    }
    Some(AppInstance {
        process_id: pid as u32,
        process_identity: marker,
        account_id: Some(identity.account_id.clone()),
        session_id: identity.session_id.map(|v| v.to_string()),
    })
}
pub(crate) fn act(
    request: &AppActionRequest,
    identity: &UserIdentity,
) -> Result<AppActionResult, AppActionError> {
    let observed = identity
        .observation(
            ExecutionMode::DesktopUser,
            ExecutionEnvironmentSource::InteractiveSession,
        )
        .map_err(|e| fail(false, e))?;
    let request = request.clone();
    let identity = identity.clone();
    let (receive, prior) = crate::on_input_thread(move || {
        let workspace = NSWorkspace::sharedWorkspace();
        let (target, file) = match &request {
            AppActionRequest::Launch { application, .. } => (Some(application), None),
            AppActionRequest::OpenFile { path, application } => (application.as_ref(), Some(path)),
        };
        let application = match target {
            Some(AppTarget::Id { id }) => Some(
                workspace
                    .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))
                    .ok_or_else(|| {
                        fail(false, "application_not_found: unknown bundle identifier")
                    })?,
            ),
            Some(AppTarget::Path { path }) => {
                let p = Path::new(path);
                if !p.is_absolute()
                    || !p.is_dir()
                    || !p.extension().is_some_and(|v| v.eq_ignore_ascii_case("app"))
                {
                    return Err(fail(
                        false,
                        "application_not_found: expected an absolute .app bundle path",
                    ));
                }
                bundle(p).map_err(|e| fail(false, e))?;
                Some(NSURL::fileURLWithPath(&NSString::from_str(path)))
            }
            None => None,
        };
        let prior: Vec<_> = workspace
            .runningApplications()
            .iter()
            .take(4096)
            .filter_map(|app| instance(&app, &identity))
            .collect();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let completion =
            block2::RcBlock::new(move |app: *mut NSRunningApplication, error: *mut NSError| {
                // The system keeps callback arguments alive until this invocation
                // returns; convert immediately to owned protocol values.
                let result = if let Some(error) = unsafe { error.as_ref() } {
                    Err(fail(true, error.localizedDescription()))
                } else if let Some(app) = unsafe { app.as_ref() } {
                    Ok(instance(app, &identity))
                } else {
                    Err(fail(true, "application activation returned no result"))
                };
                let _ = send.send(result);
            });
        let config = NSWorkspaceOpenConfiguration::configuration();
        config.setActivates(true);
        config.setCreatesNewApplicationInstance(matches!(
            request,
            AppActionRequest::Launch {
                new_instance: true,
                ..
            }
        ));
        config.setAddsToRecentItems(false);
        match (application, file) {
            (Some(app), None) => workspace.openApplicationAtURL_configuration_completionHandler(
                &app,
                &config,
                Some(&completion),
            ),
            (Some(app), Some(path)) => {
                let file = NSURL::fileURLWithPath(&NSString::from_str(path));
                let files = NSArray::from_slice(&[&*file]);
                workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                    &files,
                    &app,
                    &config,
                    Some(&completion),
                );
            }
            (None, Some(path)) => workspace.openURL_configuration_completionHandler(
                &NSURL::fileURLWithPath(&NSString::from_str(path)),
                &config,
                Some(&completion),
            ),
            (None, None) => unreachable!(),
        }
        Ok((receive, prior))
    })?;
    let instance=receive.recv_timeout(std::time::Duration::from_secs(15)).map_err(|_|fail(true,"application_activation_unconfirmed: completion was not observed; inspect the original request rather than launching again"))??;
    let reused = instance.as_ref().and_then(|v| {
        prior
            .iter()
            .any(|p| p.process_id == v.process_id && p.process_identity == v.process_identity)
            .then_some(true)
    });
    Ok(AppActionResult{request_accepted:true,instance,reused_instance:reused,execution_identity:observed,window_ready:None,notes:vec!["NSWorkspace completion does not prove a ready document/window; observe with existing window tools".into()]})
}
