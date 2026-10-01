#[path = "pab_mcp/container.rs"]
mod mcp_container;
#[path = "pab_mcp/git.rs"]
mod mcp_git;
#[path = "pab_mcp/system_query.rs"]
mod mcp_system_query;
use std::{env, path::PathBuf, sync::Arc, time::Duration};

#[path = "pab_mcp/catalog.rs"]
mod mcp_catalog;
#[path = "pab_mcp/desktop.rs"]
mod mcp_desktop;
#[path = "pab_mcp/filesystem.rs"]
mod mcp_filesystem;
#[path = "pab_mcp/filesystem_bulk.rs"]
mod mcp_filesystem_bulk;
#[path = "pab_mcp/operations.rs"]
mod mcp_operations;
#[path = "pab_mcp/screenshot.rs"]
mod mcp_screenshot;
#[path = "pab_mcp/settings.rs"]
mod mcp_settings;
#[path = "pab_mcp/tools.rs"]
mod mcp_tools;

use pab_agent_core::{DataPaths, DataScope};
use pab_bridge::desktop_presence::{McpReporter, RuntimeReport};
use pab_bridge::{
    BridgeConfig, BridgeRuntime, BridgeRuntimeConfig, MemoryDevicePasswordProvider,
    SqliteDevicePasswordProvider,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::stdio,
};
use serde_json::{Value, json};

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTRUCTIONS: &str = "Select a device by its 9-digit code. Call pab_connect before commands and preserve its verified OS/shell context across context compaction. Windows, Linux and macOS have different commands. Use pab_run_command for native OS fallback. Upload/download return an operation_ref immediately; file_hash and bulk file/ZIP operations return after remote acceptance and execute in the background. Cancellation does not roll back completed filesystem effects. Use the dedicated file tools before shell fallback, and preserve expected_hash when editing or continuing reads; query pab_get_operation with its device_code and operation_id. Reuse request_id for deduplication. An unconfirmed result is not failure: query the original request, never replay it with a new ID. cancel_requested is intent; only a terminal cancelled result confirms it stopped. pab_disconnect affects only this MCP session. The device password stays in the local Bridge database; never pass it as a tool argument.";

fn main() {
    if let Err(error) = mcp_settings::apply_at_start() {
        eprintln!("pab-mcp: {error}");
        std::process::exit(1);
    }
    let tokio = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("could not start the MCP runtime");
    tokio.block_on(async_main());
}

async fn async_main() {
    let log_root = match DataPaths::for_scope(DataScope::User) {
        Ok(paths) => paths.root().to_path_buf(),
        Err(error) => {
            eprintln!("pab-mcp: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = pab_logging::init("mcp", &log_root) {
        eprintln!("pab-mcp: {error}");
        std::process::exit(1);
    }
    if let Err(error) = run().await {
        tracing::error!(%error, "MCP process failed");
        eprintln!("pab-mcp: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if env::args_os().len() != 1 {
        return Err("pab-mcp accepts no command-line arguments".into());
    }
    let (reporter, reporting) = McpReporter::start();
    let server = McpServer {
        runtime: Default::default(),
        reporter: reporter.clone(),
        runtime_reporting: Default::default(),
        operations: Default::default(),
    };
    let result = async {
        let service = server.clone().serve(stdio()).await?;
        if let Some(info) = service.peer().peer_info() {
            reporter.set_client(
                info.client_info.name.clone(),
                info.client_info.version.clone(),
            );
        }
        service.waiting().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if let Some(operations) = server.operations.lock().await.take() {
        operations.shutdown().await;
    }
    reporting.shutdown().await;
    if let Some(task) = server.runtime_reporting.lock().await.take() {
        task.abort();
    }
    if let Some(runtime) = server.runtime.lock().await.take() {
        Arc::try_unwrap(runtime)
            .map_err(|_| "MCP runtime is still in use after shutdown")?
            .shutdown()
            .await?;
    }
    result
}

#[derive(Clone)]
struct McpServer {
    runtime: Arc<tokio::sync::Mutex<Option<Arc<BridgeRuntime>>>>,
    reporter: McpReporter,
    runtime_reporting: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    operations: Arc<tokio::sync::Mutex<Option<Arc<mcp_operations::OperationManager>>>>,
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("pixels-agent-bridge", SERVER_VERSION))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        serde_json::from_value(json!({ "tools": mcp_catalog::tools() }))
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let mut call = self.reporter.begin_call(&request.name, &arguments);
        let mut image_content = None;
        let result = if let Err(error) = mcp_catalog::validate_arguments(&request.name, &arguments)
        {
            Err(error)
        } else if request.name == "pab_list_devices" {
            mcp_tools::list_local_devices().await
        } else if mcp_operations::handles(&request.name) {
            match self.operation_manager().await {
                Ok(manager) => manager.call(&request.name, &arguments).await,
                Err(error) => Err(error),
            }
        } else {
            match self.operation_manager().await {
                Err(error) => Err(error),
                Ok(manager) => match manager.runtime().await {
                    Ok(runtime) => {
                        let result = if request.name == "pab_capture_screenshot" {
                            mcp_screenshot::call(&runtime, &arguments).await.map(
                                |(metadata, image)| {
                                    image_content = image;
                                    metadata
                                },
                            )
                        } else if mcp_desktop::handles(&request.name) {
                            mcp_desktop::call(&runtime, &request.name, &arguments).await
                        } else {
                            mcp_tools::call_tool(&runtime, &request.name, &arguments).await
                        };
                        if let Ok(value) = &result
                            && let Some(id) =
                                value.pointer("/task/request_id").and_then(Value::as_str)
                            && let Ok(id) = id.parse()
                        {
                            manager
                                .queue
                                .track_task(id)
                                .await
                                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                        }
                        result
                    }
                    Err(error) => {
                        self.reporter.set_runtime(RuntimeReport {
                            control_phase: "initialization_failed".to_owned(),
                            last_error: Some(error.clone()),
                            ..Default::default()
                        });
                        Err(error)
                    }
                },
            }
        };
        if let Ok(value) = &result {
            call.observe_result(value);
        }
        let tool_failed = result.as_ref().is_ok_and(|value| {
            matches!(
                request.name.as_ref(),
                "pab_list_monitors"
                    | "pab_list_windows"
                    | "pab_focus_window"
                    | "pab_window_control"
                    | "pab_type_text"
                    | "pab_file_stat"
                    | "pab_file_read"
                    | "pab_file_write"
                    | "pab_file_patch"
                    | "pab_file_search"
                    | "pab_list_network_connections"
                    | "pab_resolve_dns"
                    | "pab_list_sessions"
                    | "pab_terminate_process"
                    | "pab_list_services"
                    | "pab_get_service"
                    | "pab_service_control"
                    | "pab_system_info"
                    | "pab_list_disks"
                    | "pab_list_processes"
                    | "pab_get_process"
                    | "pab_list_network_interfaces"
                    | "pab_file_hash"
                    | "pab_mkdir"
                    | "pab_file_copy"
                    | "pab_file_move"
                    | "pab_file_delete"
                    | "pab_archive_create"
                    | "pab_archive_extract"
            ) && value.pointer("/result/state").and_then(Value::as_str) == Some("failed")
        });
        call.finish(result.is_ok() && !tool_failed);
        Ok(match result {
            Ok(value) => {
                let mut response = CallToolResult::structured(value);
                if let Some(image) = image_content {
                    response.content.push(image);
                }
                if tool_failed {
                    response.is_error = Some(true);
                }
                response.into()
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]).into(),
        })
    }
}

impl McpServer {
    async fn operation_manager(&self) -> Result<Arc<mcp_operations::OperationManager>, String> {
        let mut manager = self.operations.lock().await;
        if manager.is_none() {
            *manager = Some(
                mcp_operations::OperationManager::new(
                    self.runtime.clone(),
                    self.runtime_reporting.clone(),
                    self.reporter.clone(),
                )
                .await?,
            );
        }
        Ok(manager.as_ref().unwrap().clone())
    }
}

async fn ensure_runtime(
    runtime: &mut Option<Arc<BridgeRuntime>>,
    session_id: &str,
) -> Result<Arc<BridgeRuntime>, String> {
    if runtime.is_none() {
        let config = if env::var("PAB_MCP_GUEST").as_deref() != Ok("0") {
            tokio::time::timeout(
                Duration::from_secs(12),
                BridgeConfig::register_guest_from_env(),
            )
            .await
            .map_err(|_| {
                "The control server is unavailable; retry the tool call when it is online"
                    .to_owned()
            })?
            .map_err(|error| error.to_string())?
        } else {
            BridgeConfig::from_env().map_err(|error| error.to_string())?
        };
        let paths = DataPaths::for_scope(DataScope::User).map_err(|error| error.to_string())?;
        let database_path = env::var_os("PAB_BRIDGE_DATABASE")
            .map(PathBuf::from)
            .unwrap_or_else(|| paths.bridge_database());
        let passwords = Arc::new(MemoryDevicePasswordProvider::new(Some(Box::new(
            SqliteDevicePasswordProvider::new(&database_path),
        ))));
        let mut runtime_config = BridgeRuntimeConfig::new(database_path);
        runtime_config.resume_incomplete_on_start = false;
        runtime_config.session_id = Some(session_id.to_owned());
        *runtime = Some(Arc::new(
            BridgeRuntime::start(config, runtime_config, passwords)
                .await
                .map_err(|error| error.to_string())?,
        ));
    }
    Ok(runtime
        .as_ref()
        .expect("Bridge Runtime was initialized")
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_compatible_with_sdk() {
        let catalog: ListToolsResult =
            serde_json::from_value(json!({ "tools": mcp_catalog::tools() })).unwrap();
        assert_eq!(catalog.tools.len(), 60);
        let names = catalog
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert!(names.contains(&"pab_run_command"));
    }
}
