use super::{SessionBatch, bounded};
use pab_protocol::OsSessionInfo;
use zbus_systemd::{
    login1::{ManagerProxy, SessionProxy},
    zbus,
};
fn optional<T>(field: &str, value: zbus::Result<T>, errors: &mut Vec<String>) -> Option<T> {
    match value {
        Ok(v) => Some(v),
        Err(e) => {
            errors.push(format!("{field}: {}", bounded(&e.to_string())));
            None
        }
    }
}
pub(super) async fn collect() -> Result<SessionBatch, String> {
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("system bus unavailable: {e}"))?;
    let manager = ManagerProxy::new(&connection)
        .await
        .map_err(|e| e.to_string())?;
    let mut sessions = manager
        .list_sessions()
        .await
        .map_err(|e| format!("logind unavailable: {e}"))?;
    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    let truncated = sessions.len() > 4096;
    let mut entries = vec![];
    for (id, uid, user, seat, path) in sessions.into_iter().take(4096) {
        let mut errors = vec![];
        let mut info = OsSessionInfo {
            id: bounded(&id),
            user_name: (!user.is_empty()).then(|| bounded(&user)),
            user_id: Some(uid),
            domain: None,
            state: "unknown".into(),
            active: None,
            remote: None,
            seat: (!seat.is_empty()).then(|| bounded(&seat)),
            terminal: None,
            client_name: None,
            session_type: None,
            errors: vec![],
        };
        let proxy = SessionProxy::builder(&connection)
            .path(path)
            .map_err(|e| e.to_string())?
            .build()
            .await;
        match proxy {
            Ok(s) => {
                info.state = optional("state", s.state().await, &mut errors)
                    .map(|s| bounded(&s))
                    .unwrap_or_else(|| "unknown".into());
                info.active = optional("active", s.active().await, &mut errors);
                info.remote = optional("remote", s.remote().await, &mut errors);
                info.terminal = optional("tty", s.tty().await, &mut errors)
                    .filter(|s| !s.is_empty())
                    .map(|s| bounded(&s));
                info.client_name = optional("remote_host", s.remote_host().await, &mut errors)
                    .filter(|s| !s.is_empty())
                    .map(|s| bounded(&s));
                info.session_type =
                    optional("type", s.type_property().await, &mut errors).map(|s| bounded(&s));
            }
            Err(e) => errors.push(format!(
                "session disappeared or inaccessible: {}",
                bounded(&e.to_string())
            )),
        }
        info.errors = errors;
        entries.push(info);
    }
    Ok(SessionBatch {
        backend: "linux_logind",
        entries,
        truncated,
    })
}
