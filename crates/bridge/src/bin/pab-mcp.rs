use std::{env, path::PathBuf, sync::Arc, time::Duration};

#[path = "pab_mcp/catalog.rs"]
mod mcp_catalog;
#[path = "pab_mcp/settings.rs"]
mod mcp_settings;
#[path = "pab_mcp/tools.rs"]
mod mcp_tools;

use pab_agent_core::{DataPaths, DataScope};
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
const INSTRUCTIONS: &str = "Select a device by its 9-digit code. Call pab_connect before commands and preserve its verified OS/shell context across context compaction. Windows, Linux and macOS have different commands. Use pab_run_command for native OS fallback. The device password stays in the local Bridge database; never pass it as a tool argument.";

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
    let server = McpServer::default();
    server.clone().serve(stdio()).await?.waiting().await?;
    if let Some(runtime) = server.runtime.lock().await.take() {
        runtime.shutdown().await?;
    }
    Ok(())
}

#[derive(Clone, Default)]
struct McpServer {
    runtime: Arc<tokio::sync::Mutex<Option<BridgeRuntime>>>,
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
        let result = if request.name == "pab_list_devices" {
            mcp_tools::list_local_devices().await
        } else {
            let mut runtime = self.runtime.lock().await;
            match ensure_runtime(&mut runtime).await {
                Ok(runtime) => mcp_tools::call_tool(runtime, &request.name, &arguments).await,
                Err(error) => Err(error),
            }
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value).into(),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]).into(),
        })
    }
}

async fn ensure_runtime(runtime: &mut Option<BridgeRuntime>) -> Result<&BridgeRuntime, String> {
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
        *runtime = Some(
            BridgeRuntime::start(config, runtime_config, passwords)
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(runtime.as_ref().expect("Bridge Runtime was initialized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_compatible_with_sdk() {
        let catalog: ListToolsResult =
            serde_json::from_value(json!({ "tools": mcp_catalog::tools() })).unwrap();
        assert_eq!(catalog.tools.len(), 16);
        let names = catalog
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert!(names.contains(&"pab_run_command"));
    }
}
