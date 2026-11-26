//! Model Context Protocol (MCP) server & Command + Capability Guard dispatcher.
//!
//! Designed so external AI agents (Gemini CLI, Antigravity, ChatGPT, Claude,
//! Codex) can query digests or invoke desktop controls safely without ever
//! accessing decrypted credentials.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use digest_core::domain::{McpAccessMode, McpToolName};
use digest_core::error::CoreError;
use serde_json::Value;

/// Command trait implemented by each MCP tool.
#[async_trait]
pub trait McpToolHandler: Send + Sync {
    /// Canonical tool identifier.
    fn name(&self) -> McpToolName;

    /// Minimum capability mode required to invoke this tool.
    fn required_access(&self) -> McpAccessMode;

    /// Executes the tool with JSON arguments.
    async fn execute(&self, args: Value) -> Result<Value, CoreError>;
}

/// Central MCP tool registry enforcing `McpAccessMode` before execution.
#[derive(Default)]
pub struct McpDispatcher {
    handlers: HashMap<McpToolName, Arc<dyn McpToolHandler>>,
}

impl McpDispatcher {
    /// Registers a tool handler by its canonical `McpToolName`.
    pub fn register(&mut self, handler: Arc<dyn McpToolHandler>) {
        self.handlers.insert(handler.name(), handler);
    }

    /// Dispatches a tool call only if `granted_mode` satisfies the tool's guard.
    pub async fn dispatch(
        &self,
        granted_mode: McpAccessMode,
        tool: McpToolName,
        args: Value,
    ) -> Result<Value, CoreError> {
        let handler = self
            .handlers
            .get(&tool)
            .ok_or_else(|| CoreError::NotFound(format!("unregistered MCP tool: {tool}")))?;

        let required = handler.required_access();
        if !granted_mode.allows(required) {
            return Err(CoreError::Forbidden(format!(
                "MCP tool '{tool}' requires '{required}' access (current mode: '{granted_mode}')"
            )));
        }

        handler.execute(args).await
    }
}

#[cfg(test)]
mod tests {
    use digest_core::domain::ErrorCode;
    use serde_json::json;

    use super::*;

    struct StubReadTool;

    #[async_trait]
    impl McpToolHandler for StubReadTool {
        fn name(&self) -> McpToolName {
            McpToolName::DigestGetDaily
        }

        fn required_access(&self) -> McpAccessMode {
            McpAccessMode::ReadOnly
        }

        async fn execute(&self, _args: Value) -> Result<Value, CoreError> {
            Ok(json!({ "ok": true }))
        }
    }

    struct StubWriteTool;

    #[async_trait]
    impl McpToolHandler for StubWriteTool {
        fn name(&self) -> McpToolName {
            McpToolName::DigestTriggerRun
        }

        fn required_access(&self) -> McpAccessMode {
            McpAccessMode::ReadWrite
        }

        async fn execute(&self, _args: Value) -> Result<Value, CoreError> {
            Ok(json!({ "triggered": true }))
        }
    }

    #[tokio::test]
    async fn capability_guard_blocks_unauthorized_modes() {
        let mut dispatcher = McpDispatcher::default();
        dispatcher.register(Arc::new(StubReadTool));
        dispatcher.register(Arc::new(StubWriteTool));

        let disabled_err = dispatcher
            .dispatch(
                McpAccessMode::Disabled,
                McpToolName::DigestGetDaily,
                json!({}),
            )
            .await
            .unwrap_err();
        assert_eq!(disabled_err.error_code(), ErrorCode::Forbidden);

        let ro_ok = dispatcher
            .dispatch(
                McpAccessMode::ReadOnly,
                McpToolName::DigestGetDaily,
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(ro_ok["ok"], true);

        let ro_write_err = dispatcher
            .dispatch(
                McpAccessMode::ReadOnly,
                McpToolName::DigestTriggerRun,
                json!({}),
            )
            .await
            .unwrap_err();
        assert_eq!(ro_write_err.error_code(), ErrorCode::Forbidden);

        let rw_ok = dispatcher
            .dispatch(
                McpAccessMode::ReadWrite,
                McpToolName::DigestTriggerRun,
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(rw_ok["triggered"], true);
    }
}
