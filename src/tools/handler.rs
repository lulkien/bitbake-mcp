//! The MCP `ServerHandler`: server info, and tool dispatch with timing.

use super::BitBakeServer;
use super::envelope::with_elapsed_ms;
use crate::{SERVER_NAME, SERVER_VERSION};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, tool_handler,
    handler::server::tool::ToolCallContext,
    model::{CallToolRequestParams, CallToolResponse, ServerCapabilities, ServerConfig},
};
use std::time::Instant;

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BitBakeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(SERVER_NAME, SERVER_VERSION))
            .with_instructions(
                "Deterministic BitBake/Yocto build server: runs bitbake and devtool \
                 commands in a Yocto build directory.",
            )
    }

    /// Dispatch the tool call and let the result carry its own duration.
    ///
    /// This is the one place every tool call passes through, so timing here
    /// cannot be forgotten by a tool or skipped by an early validation return.
    /// `#[tool_handler]` only generates `call_tool` when the impl block does not
    /// define one, so this override replaces the generated dispatch with the
    /// same dispatch plus the measurement.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let started = Instant::now();
        let ct = context.ct.clone();
        let tcc = ToolCallContext::new(self, request, context);
        // rmcp only signals a cancelled request through `ct`; dropping the
        // dispatch future here drops the running child, which `kill_on_drop`
        // then kills.
        let response = tokio::select! {
            r = self.tool_router.call(tcc) => r?,
            _ = ct.cancelled() => {
                return Err(ErrorData::internal_error("request cancelled", None));
            }
        };
        Ok(with_elapsed_ms(response, started.elapsed().as_millis()))
    }
}
