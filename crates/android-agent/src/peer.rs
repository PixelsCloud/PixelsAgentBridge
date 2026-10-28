//! Existing PAB transport and shared password gate, followed by mobile-only execution.
use crate::{
    broker::{PlatformBroker, ReferenceScope},
    config::AndroidConfig,
};
use pab_agent_core::{DevicePeerAuthorizer, EndpointControlSupervisorHandle};
use pab_device_session::{ActiveSessions, DevicePassword, PasswordAttempts};
use pab_protocol::*;
use pab_transport::{PabConnection, PabEndpoint, PabEndpointConfig};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOptions {
    pub database_file: String,
    pub password_hash: String,
    pub password_version: u64,
    pub android_release: String,
    pub android_sdk: u32,
    #[serde(default)]
    pub storage_staging_directory: Option<String>,
}

pub struct PeerRuntime {
    pub endpoint: PabEndpoint,
    pub broker: Arc<PlatformBroker>,
    pub files: Option<Arc<crate::filesystem::FileService>>,
    pub system: Option<Arc<crate::system::SystemService>>,
    pub documents: Option<Arc<crate::documents::DocumentService>>,
    pub execution: Option<Arc<crate::execution::ExecutionService>>,
    password: Arc<std::sync::RwLock<DevicePassword>>,
    task: JoinHandle<()>,
}

pub async fn bind(
    config: &AndroidConfig,
    key: iroh_base::SecretKey,
) -> Result<PabEndpoint, &'static str> {
    let relays = config
        .relay_urls
        .iter()
        .map(|url| url.parse().map_err(|_| "invalid_relay_url"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut endpoint = PabEndpointConfig::new(relays).map_err(|_| "invalid_relay_url")?;
    if let Some(pem) = &config.trusted_ca_pem {
        endpoint = endpoint
            .with_extra_ca_pem(pem.as_bytes())
            .map_err(|_| "invalid_ca")?;
    }
    #[cfg(debug_assertions)]
    if config.relay_only_for_testing {
        endpoint = endpoint.relay_only_for_testing();
    }
    PabEndpoint::bind(endpoint, key)
        .await
        .map_err(|_| "transport_unavailable")
}

pub fn context(options: &RuntimeOptions) -> Result<ExecutionContext, &'static str> {
    if options.android_release.len() > 64 || !(28..=100).contains(&options.android_sdk) {
        return Err("unsupported_android");
    }
    Ok(ExecutionContext {
        os_family: OsFamily::Android,
        os_name: "Android".into(),
        os_version: options.android_release.clone(),
        architecture: if cfg!(target_arch = "aarch64") {
            CpuArchitecture::Aarch64
        } else {
            CpuArchitecture::X86_64
        },
        execution_scope: ExecutionScope::Native,
        identity: None,
        path_style: PathStyle::Posix,
        interpreter: None,
        cwd: options
            .storage_staging_directory
            .as_ref()
            .map(|_| "/workspace".into()),
        environment_revision: format!(
            "android:{}:{}",
            options.android_sdk,
            env!("CARGO_PKG_VERSION")
        ),
    })
}

pub fn network(endpoint: &PabEndpoint, device: DeviceRef) -> watch::Receiver<DeviceNetworkUpdate> {
    let mut addresses = endpoint.watch_address();
    let instance = EndpointInstanceId::new();
    let update = |address: pab_transport::PabEndpointAddress, revision| DeviceNetworkUpdate {
        schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
        device_ref: device,
        endpoint_key: EndpointKey::new(*endpoint.id().as_bytes()),
        endpoint_instance_id: instance,
        address_revision: revision,
        relay_urls: address.relay_urls,
        direct_addresses: address.direct_addresses,
        observed_at_unix_ms: now_ms(),
    };
    let (sender, receiver) = watch::channel(update(addresses.borrow().clone(), 1));
    let key = *endpoint.id().as_bytes();
    tokio::spawn(async move {
        let mut revision = 1;
        while addresses.changed().await.is_ok() {
            revision += 1;
            let address = addresses.borrow_and_update().clone();
            if sender
                .send(DeviceNetworkUpdate {
                    schema_version: DEVICE_NETWORK_SCHEMA_VERSION,
                    device_ref: device,
                    endpoint_key: EndpointKey::new(key),
                    endpoint_instance_id: instance,
                    address_revision: revision,
                    relay_urls: address.relay_urls,
                    direct_addresses: address.direct_addresses,
                    observed_at_unix_ms: now_ms(),
                })
                .is_err()
            {
                break;
            }
        }
    });
    receiver
}

impl PeerRuntime {
    pub async fn start(
        endpoint: PabEndpoint,
        supervisor: &EndpointControlSupervisorHandle,
        device: DeviceRef,
        execution: ExecutionContext,
        options: RuntimeOptions,
        digest_key: [u8; 32],
    ) -> Result<Self, &'static str> {
        let password = Arc::new(std::sync::RwLock::new(
            DevicePassword::from_phc(options.password_version, options.password_hash)
                .map_err(|_| "invalid_access_password")?,
        ));
        let credentials = Arc::clone(&password);
        let path = Path::new(&options.database_file);
        if !path.is_absolute() {
            return Err("invalid_runtime_path");
        }
        let broker = PlatformBroker::open(path, digest_key).await?;
        let system = crate::system::SystemService::open(
            &path.with_extension("system.sqlite"),
            digest_key,
            device,
        )
        .await
        .ok();
        let system_service = system.clone();
        let files = match options.storage_staging_directory {
            Some(directory) => Some(
                crate::filesystem::FileService::open(
                    &path.with_extension("files.sqlite"),
                    Path::new(&directory),
                    digest_key,
                )
                .await?,
            ),
            None => None,
        };
        let file_service = files.clone();
        let documents = match &files {
            Some(files) => crate::documents::DocumentService::open(
                &path.with_extension("documents.sqlite"),
                digest_key,
                files.clone(),
            )
            .await
            .ok(),
            None => None,
        };
        let document_service = documents.clone();
        let commands = crate::execution::ExecutionService::open(
            &path.with_extension("execution.sqlite"),
            digest_key,
            device,
        )
        .await
        .ok();
        let command_service = commands.clone();
        let forwards = commands
            .as_ref()
            .map(|service| crate::port_forward::PortForwardService::new(service.clone()));
        endpoint.follow_relay_updates(supervisor.relay_updates());
        let authorizer = supervisor.peer_authorizer();
        let listener = endpoint.clone();
        let platform = Arc::clone(&broker);
        let task = tokio::spawn(async move {
            let attempts = PasswordAttempts::default();
            let active = ActiveSessions::default();
            let limit = Arc::new(Semaphore::new(16));
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    incoming=listener.accept()=>{
                        let Ok(Some(connection))=incoming else {break};
                        let Ok(permit)=Arc::clone(&limit).try_acquire_owned() else {connection.close(b"busy");continue};
                        let authorizer=authorizer.clone();let credential=credentials.read().expect("access credential lock").clone();let attempts=attempts.clone();let broker=Arc::clone(&platform);let context=SessionContext {execution:execution.clone(),broker,active:active.clone(),files:file_service.clone(),system:system_service.clone(),documents:document_service.clone(),commands:command_service.clone(),forwards:forwards.clone()};
                        sessions.spawn(async move {let _permit=permit;let _=session(connection,authorizer,credential,attempts,device,context).await;});
                    }
                    _=sessions.join_next(), if !sessions.is_empty()=>{}
                }
            }
        });
        Ok(Self {
            endpoint,
            broker,
            files,
            system,
            documents,
            execution: commands,
            password,
            task,
        })
    }
    pub fn update_password(&self, credential: DevicePassword) -> Result<(), &'static str> {
        *self
            .password
            .write()
            .map_err(|_| "credential_unavailable")? = credential;
        Ok(())
    }
    pub async fn stop(self) {
        if let Some(execution) = &self.execution {
            execution.shutdown().await;
        }
        self.task.abort();
        let _ = self.task.await;
        self.endpoint.close().await;
    }
}

#[derive(Clone)]
struct SessionContext {
    forwards: Option<Arc<crate::port_forward::PortForwardService>>,
    commands: Option<Arc<crate::execution::ExecutionService>>,
    documents: Option<Arc<crate::documents::DocumentService>>,
    system: Option<Arc<crate::system::SystemService>>,
    execution: ExecutionContext,
    broker: Arc<PlatformBroker>,
    active: ActiveSessions,
    files: Option<Arc<crate::filesystem::FileService>>,
}

async fn session(
    connection: PabConnection,
    authorizer: DevicePeerAuthorizer,
    credential: DevicePassword,
    attempts: PasswordAttempts,
    device: DeviceRef,
    context: SessionContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let timeout = Duration::from_secs(8);
    let peer = match pab_device_session::authenticate(
        &connection,
        &authorizer,
        &credential,
        device,
        timeout,
        &attempts,
    )
    .await
    {
        Ok(peer) => peer,
        Err(error) => {
            // Let the peer consume the explicit rejection before closing QUIC. An
            // immediate close can overtake the stream and look like a network failure.
            let _ = tokio::time::timeout(Duration::from_secs(1), connection.closed()).await;
            connection.close(b"authentication rejected");
            return Err(error.into());
        }
    };
    let _presence = context.active.enter(peer.peer_endpoint_key);
    let broker = context.broker;
    let files = context.files;
    let system = context.system;
    let documents = context.documents;
    let commands = context.commands;
    let forwards = context.forwards;
    let forward_slots = Arc::new(Semaphore::new(4));
    let command_lease = commands.as_ref().map(|service| service.connection_lease());
    let command_scope = command_lease.as_ref().map(|lease| lease.scope.clone());
    let system_scope = uuid::Uuid::new_v4().to_string();
    let references = ReferenceScope::default();
    let _lease = references.lease();
    let scope = Arc::new(Mutex::new(references));
    let user = peer.user_context.user.as_ref().map(|user| user.user_id);
    let owner = format!(
        "{}:user:{}",
        peer.operator.storage_key(),
        user.map(|id| id.to_string())
            .unwrap_or_else(|| "none".into())
    );
    let mut streams = JoinSet::new();
    let limit = Arc::new(Semaphore::new(16));
    let mut recheck = tokio::time::interval(Duration::from_secs(5));
    recheck.tick().await;
    loop {
        tokio::select! {
            _=recheck.tick()=>{
                if !matches!(authorizer.report_authenticated(peer.peer_endpoint_key).await,Ok(current) if current.operator==peer.operator && current.device_ref==device && current.peer_endpoint_key==peer.peer_endpoint_key && current.user_context.user.as_ref().map(|value|value.user_id)==user){connection.close(b"authorization expired");break;}
            }
            incoming=connection.accept_bi(Duration::from_secs(60))=>match incoming {
                Ok(mut stream)=>{
                    let Ok(permit)=Arc::clone(&limit).try_acquire_owned() else {connection.close(b"busy");break};
                    let Ok(current)=authorizer.authorize(peer.peer_endpoint_key).await else {connection.close(b"authorization expired");break};
                    if current.operator!=peer.operator || current.device_ref!=device || current.peer_endpoint_key!=peer.peer_endpoint_key || current.user_context.user.as_ref().map(|value|value.user_id)!=user {connection.close(b"identity mismatch");break;}
                    let broker=Arc::clone(&broker);let scope=Arc::clone(&scope);let owner=owner.clone();let execution=context.execution.clone();let active=context.active.clone();let files=files.clone();let system=system.clone();let system_scope=system_scope.clone();let documents=documents.clone();let commands=commands.clone();let command_scope=command_scope.clone();let forwards=forwards.clone();let forward_slots=forward_slots.clone();
                    streams.spawn(async move {
                        let _permit=permit;
                        let request:DeviceTaskRequest=stream.receive_json(timeout).await?;
                        if request.schema_version()==DEVICE_TASK_SCHEMA_VERSION
                            && let (Some(service),DeviceTaskRequest::PortForwardConnect {port_forward_schema_version,host,port,..})=(&forwards,&request) {
                            return service.serve(stream,forward_slots,*port_forward_schema_version,host,*port,timeout).await;
                        }
                        if request.schema_version()==DEVICE_TASK_SCHEMA_VERSION
                            && let (Some(service),Some(scope))=(&commands,&command_scope)
                            && service.serve_request(&owner,scope,&current,&execution,&request,&mut stream).await? {return Ok(());}
                        if request.schema_version()==DEVICE_TASK_SCHEMA_VERSION
                            && let Some(service)=&files
                            && service.serve_request(&owner,&current,&execution,&request,&mut stream).await? {
                            return Ok(());
                        }
                        let response=if request.schema_version()!=DEVICE_TASK_SCHEMA_VERSION {unsupported()} else { match request {
                            DeviceTaskRequest::GetEnvironment {..}=>DeviceTaskResponse::Environment {
                                context:Box::new(TargetContext {device_ref:device,execution,source:TargetContextSource::ExecutorVerified,observed_at_unix_ms:now_ms(),freshness:ContextFreshness::Current}),
                                command_schema_version:commands.as_ref().map(|_|2),terminal_schema_version:commands.as_ref().map(|_|1),desktop_terminal_schema_version:None,remote_desktop_schema_version:None,remote_desktop_cursor_schema_version:None,remote_desktop_clipboard_schema_version:None,filesystem_schema_version:files.as_ref().map(|_|4),transfer_schema_version:files.as_ref().map(|_|FILE_TRANSFER_NATIVE_SCHEMA_VERSION),system_query_schema_version:system.as_ref().map(|_|SYSTEM_QUERY_SCHEMA_VERSION),screenshot_schema_version:Some(SCREENSHOT_SCHEMA_VERSION),document_schema_version:documents.as_ref().map(|_|DOCUMENT_SCHEMA_VERSION),port_forward_schema_version:forwards.as_ref().map(|_|PORT_FORWARD_SCHEMA_VERSION),mobile_schema_version:Some(MOBILE_SCHEMA_VERSION),
                            },
                            DeviceTaskRequest::Document {request,..} if documents.is_some()=>DeviceTaskResponse::Document {reply:Box::new(documents.as_ref().expect("document capability").request(&owner,request).await)},
                            DeviceTaskRequest::GetDocument {request_id,..} if documents.is_some()=>DeviceTaskResponse::Document {reply:Box::new(documents.as_ref().expect("document capability").authorized_lookup(&owner,request_id).await)},
                            DeviceTaskRequest::CancelDocument {request_id,..} if documents.is_some()=>DeviceTaskResponse::Document {reply:Box::new(documents.as_ref().expect("document capability").cancel(&owner,request_id).await)},
                            DeviceTaskRequest::GetUserContext {..}=>DeviceTaskResponse::UserContext {context:current.user_context},
                            DeviceTaskRequest::GetPresence {..}=>DeviceTaskResponse::Presence {active_operators:active.operator_count()},
                            DeviceTaskRequest::SystemQuery {request_id,query,..} if system.is_some()=>DeviceTaskResponse::SystemQuery {reply:Box::new(system.as_ref().expect("system capability").request(&owner,&system_scope,request_id,query,execution).await)},
                            DeviceTaskRequest::CaptureScreenshotV2 {request_id,options,..} if system.is_some()=> {
                                match crate::screenshot::capture(&broker,system.as_ref().expect("system capability"),&owner,&system_scope,scope,request_id,options).await {
                                    Ok((meta,bytes))=> {
                                        stream.send_frame_json(&DeviceTaskResponse::Screenshot{meta},timeout).await?;
                                        for chunk in bytes.chunks(pab_transport::MAX_BINARY_FRAME_BYTES) {stream.send_binary_frame(chunk,timeout).await?;}
                                        return stream.finish_send(timeout).await;
                                    }
                                    Err(message)=>DeviceTaskResponse::Error{code:DeviceTaskErrorCode::Unsupported,message},
                                }
                            }
                            DeviceTaskRequest::GetSystemQuery {request_id,..} if system.is_some()=>DeviceTaskResponse::SystemQuery {reply:Box::new(system.as_ref().expect("system capability").lookup(&owner,request_id).await)},
                            DeviceTaskRequest::CancelSystemQuery {request_id,..} if system.is_some()=>{
                                let mut reply=system.as_ref().expect("system capability").lookup(&owner,request_id).await;
                                reply.warnings.push("Android system queries and activity dispatch cannot be cancelled; original outcome retained".into());
                                reply.warnings.truncate(8);
                                DeviceTaskResponse::SystemQuery {reply:Box::new(reply)}
                            },
                            DeviceTaskRequest::Mobile {mobile_schema_version,request,..} if (1..=MOBILE_SCHEMA_VERSION).contains(&mobile_schema_version) && request.required_version()<=mobile_schema_version=>{
                                let mut reply=broker.request(&owner,scope,request).await;
                                if mobile_schema_version==1 && let Some(observation)=reply.observation.as_mut() { observation.package_name=None; }
                                let image=if reply.image.is_some(){broker.image(&owner,reply.request_id).await}else{None};
                                if reply.image.is_some() && image.is_none(){reply.image=None;reply.error=Some("image_expired".into());}
                                stream.send_frame_json(&DeviceTaskResponse::Mobile {reply:Box::new(reply)},timeout).await?;
                                if let Some(image)=image { for chunk in image.chunks(pab_transport::MAX_BINARY_FRAME_BYTES){stream.send_binary_frame(chunk,timeout).await?;} }
                                return stream.finish_send(timeout).await;
                            }
                            _=>unsupported(),
                        }};
                        stream.send_json(&response,timeout).await
                    });
                }
                Err(pab_transport::PabConnectionError::Timeout)=>continue,
                Err(_)=>break,
            },
            _=streams.join_next(), if !streams.is_empty()=>{}
        }
    }
    streams.abort_all();
    connection.close(b"session ended");
    Ok(())
}
fn unsupported() -> DeviceTaskResponse {
    DeviceTaskResponse::Error {
        code: DeviceTaskErrorCode::Unsupported,
        message: "This Android device does not support the requested operation".into(),
    }
}
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_millis() as i64)
        .unwrap_or(0)
}
