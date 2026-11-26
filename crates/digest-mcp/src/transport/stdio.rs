//! Line-delimited JSON-RPC 2.0 `stdio` MCP transport.
//!
//! All diagnostic logs MUST go strictly to `stderr`; `stdout` is reserved
//! exclusively for JSON-RPC 2.0 messages.

use std::sync::Arc;

use digest_core::error::CoreError;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::config::McpSharedState;
use crate::transport::protocol::handle_json_rpc_line;
use crate::McpDispatcher;

/// Serves MCP JSON-RPC 2.0 over standard input/output streams until EOF.
pub async fn serve_stdio(
    dispatcher: Arc<McpDispatcher>,
    shared_state: McpSharedState,
) -> Result<(), CoreError> {
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_io(stdin, stdout, dispatcher, shared_state).await
}

/// Generic stream-backed `stdio` loop enabling deterministic in-memory duplex tests.
pub async fn serve_stdio_with_io<R, W>(
    reader: R,
    mut writer: W,
    dispatcher: Arc<McpDispatcher>,
    shared_state: McpSharedState,
) -> Result<(), CoreError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|err| CoreError::Internal(format!("failed reading stdio line: {err}")))?
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let granted_mode = shared_state.access_mode();
        if let Some(response) = handle_json_rpc_line(trimmed, &dispatcher, granted_mode).await {
            let encoded = serde_json::to_string(&response).map_err(|err| {
                CoreError::Internal(format!("failed serializing JSON-RPC response: {err}"))
            })?;
            writer.write_all(encoded.as_bytes()).await.map_err(|err| {
                CoreError::Internal(format!("failed writing stdio response: {err}"))
            })?;
            writer.write_all(b"\n").await.map_err(|err| {
                CoreError::Internal(format!("failed writing stdio newline: {err}"))
            })?;
            writer.flush().await.map_err(|err| {
                CoreError::Internal(format!("failed flushing stdio stream: {err}"))
            })?;
        }
    }
    Ok(())
}
