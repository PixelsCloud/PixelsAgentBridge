use super::*;
use crate::{
    task_service::git::{self as native, GitEvent, GitSink},
    task_store::TaskStore,
};
use pab_protocol::{
    ExecutionContext, ExecutionEnvironmentSource, ExecutionMode, GitQuery, RequestId,
    SystemQueryReply,
};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Query {
        id: RequestId,
        query: GitQuery,
        context: ExecutionContext,
    },
    Reconcile {
        query: GitQuery,
        original: SystemQueryReply,
    },
}
impl Request {
    fn query(&self) -> &GitQuery {
        match self {
            Self::Query { query, .. } | Self::Reconcile { query, .. } => query,
        }
    }
    fn id(&self) -> RequestId {
        match self {
            Self::Query { id, .. } => *id,
            Self::Reconcile { original, .. } => original.request_id,
        }
    }
    fn context(&self) -> Option<&ExecutionContext> {
        match self {
            Self::Query { context, .. } => Some(context),
            Self::Reconcile { original, .. } => original.execution_context.as_ref(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    Lock { path: PathBuf },
    Progress { snapshot: SystemQueryReply },
    Finished { snapshot: Option<SystemQueryReply> },
}

pub(super) async fn serve(mut peer: Peer, request: Request) -> io::Result<()> {
    request.query().validate().map_err(io::Error::other)?;
    let identity = current_identity()?.observation(
        ExecutionMode::User,
        ExecutionEnvironmentSource::NativeAccount,
    )?;
    if request.context().and_then(|v| v.identity.as_ref()) != Some(&identity) {
        return Err(io::Error::other("Git execution identity mismatch"));
    }
    let (send, mut events) = mpsc::channel(4);
    let (cancel, receive) = watch::channel(false);
    let mut work = AbortOnDrop(tokio::spawn(async move {
        match request {
            Request::Query { id, query, context } => Some(
                native::query_with_sink(id, &query, receive, GitSink::Worker(send), Some(context))
                    .await,
            ),
            Request::Reconcile { query, original } => {
                native::reconcile_push(&query, &original).await
            }
        }
    }));
    loop {
        tokio::select! {
            frame=peer.receive()=>match decode(frame?)? {
                Control::Cancel{..}=>{let _=cancel.send(true);},
                _=>return Err(io::Error::other("unexpected Git worker request")),
            },
            event=events.recv()=>match event {
                Some(event)=>{
                    let (reply,answer)=match event {GitEvent::Lock(path,answer)=>(Reply::Lock{path},answer),GitEvent::Progress(snapshot,answer)=>(Reply::Progress{snapshot},answer)};
                    peer.control(Control::GitReply{reply}).await?;
                    let ack=tokio::time::timeout(Duration::from_secs(30),async {
                        loop {match decode(peer.receive().await?)? {
                            Control::GitAck{error}=>return Ok::<_,io::Error>(error.map_or(Ok(()),Err)),
                            Control::Cancel{..}=>{let _=cancel.send(true);},
                            _=>return Err(io::Error::other("unexpected Git acknowledgement")),
                        }}
                    }).await.map_err(io::Error::other)??;
                    let _=answer.send(ack);
                }
                None=>{
                    let snapshot=(&mut work.0).await.map_err(io::Error::other)?;
                    peer.control(Control::GitReply{reply:Reply::Finished{snapshot}}).await?;
                    return Ok(());
                }
            }
        }
    }
}

pub(crate) async fn execute(
    executable: &Path,
    prepared: PreparedUser,
    id: RequestId,
    query: GitQuery,
    context: ExecutionContext,
    store: TaskStore,
    cancel: watch::Receiver<bool>,
) -> Result<SystemQueryReply, TaskServiceError> {
    operate(
        executable,
        prepared,
        Request::Query { id, query, context },
        Some(store),
        cancel,
    )
    .await?
    .ok_or_else(|| io::Error::other("Git worker omitted result").into())
}
pub(crate) async fn reconcile(
    executable: &Path,
    prepared: PreparedUser,
    query: GitQuery,
    original: SystemQueryReply,
) -> Result<Option<SystemQueryReply>, TaskServiceError> {
    let (_send, cancel) = watch::channel(false);
    operate(
        executable,
        prepared,
        Request::Reconcile { query, original },
        None,
        cancel,
    )
    .await
}
async fn operate(
    executable: &Path,
    prepared: PreparedUser,
    request: Request,
    store: Option<TaskStore>,
    mut cancel: watch::Receiver<bool>,
) -> Result<Option<SystemQueryReply>, TaskServiceError> {
    let id = request.id();
    let kind = request.query().kind().to_owned();
    let context = request.context().cloned();
    let duration = match &request {
        Request::Query { query, .. } => {
            Duration::from_millis(u64::from(query.timeout_ms)) + Duration::from_secs(30)
        }
        Request::Reconcile { .. } => Duration::from_secs(10),
    };
    let (mut peer, mut child) = launch(executable, prepared).await?;
    let mut repository = None;
    let result=tokio::time::timeout(duration,async {
        peer.control(Control::Git{request}).await?;
        let mut cancel_at=None;
        let initial_cancel = *cancel.borrow();
        if initial_cancel {peer.control(Control::Cancel{reason:"Git cancellation requested".into()}).await?;cancel_at=Some(tokio::time::Instant::now()+Duration::from_secs(10));}
        loop {
            let cancel_timeout=async {match cancel_at {Some(at)=>tokio::time::sleep_until(at).await,None=>std::future::pending::<()>().await}};
            let frame=tokio::select! {
                r=peer.receive()=>r?,
                _=async { let _=cancel.wait_for(|v|*v).await; },if cancel_at.is_none()=>{
                    peer.control(Control::Cancel{reason:"Git cancellation requested".into()}).await?;
                    cancel_at=Some(tokio::time::Instant::now()+Duration::from_secs(10));continue;
                },
                _=cancel_timeout=>return Err(io::Error::new(io::ErrorKind::TimedOut,"Git cancellation not confirmed")),
            };
            let Control::GitReply{reply}=decode(frame)? else {return Err(io::Error::other("unexpected Git worker response"));};
            match reply {
                Reply::Lock{path}=>{
                    if repository.is_some() || !path.is_absolute() || path.as_os_str().len()>32768 {return Err(io::Error::other("invalid Git lock request"));}
                    let result=native::repo_lock(path).await.try_lock_owned();
                    let error=match result {Ok(lock)=>{repository=Some(lock);None},Err(_)=>Some("git_repository_busy: another PAB Git operation is active".into())};
                    peer.control(Control::GitAck{error}).await?;
                }
                Reply::Progress{snapshot}=>{
                    validate_reply(&snapshot,id,&kind,&context)?;
                    let discovering=matches!(&snapshot.data,Some(pab_protocol::SystemQueryData::Git {snapshot}) if snapshot.phase=="resolving_repository" && snapshot.git_dir.is_empty() && !snapshot.index_may_have_changed && !snapshot.command_completed);
                    if snapshot.state!="running" || (repository.is_none() && !discovering) {return Err(io::Error::other("invalid Git progress phase"));}
                    let Some(store)=&store else {return Err(io::Error::other("unexpected reconciliation progress"));};
                    store.save_system_progress(&snapshot).await.map_err(io::Error::other)?;
                    peer.control(Control::GitAck{error:None}).await?;
                }
                Reply::Finished{snapshot}=>{
                    if let Some(reply)=&snapshot {validate_reply(reply,id,&kind,&context)?;if !["completed","failed","cancelled","unconfirmed"].contains(&reply.state.as_str()){return Err(io::Error::other("invalid Git terminal state"));}}
                    return Ok(snapshot);
                }
            }
        }
    }).await.map_err(io::Error::other).and_then(|v|v);
    drop(peer);
    // Finished is the worker's final response; no further Git work is allowed.
    // Kill its owned tree before releasing the repository lock. On Windows,
    // inherited pipe reads can keep the worker runtime alive while a credential
    // helper continues running, so waiting for a graceful exit first is unsafe.
    // The same cleanup is required when the IPC exchange fails or times out.
    let cleanup = child.terminate();
    if cleanup.is_err() {
        let _ = child.terminate();
    }
    drop(child);
    drop(repository);
    cleanup?;
    result.map_err(Into::into)
}
fn validate_reply(
    reply: &SystemQueryReply,
    id: RequestId,
    kind: &str,
    context: &Option<ExecutionContext>,
) -> io::Result<()> {
    if reply.request_id != id || reply.kind != kind || &reply.execution_context != context {
        return Err(io::Error::other("Git result identity mismatch"));
    }
    Ok(())
}
