//! Standalone `digest-mcp` CLI binary supporting `stdio` and local `127.0.0.1` HTTP transports.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::process::ExitCode;
use std::str::FromStr;
use std::sync::Arc;

use digest_core::constants::SERVICE_NAME;
use digest_core::domain::McpAccessMode;
use digest_core::error::CoreError;
use digest_mcp::client::DigestApiClient;
use digest_mcp::config::{
    validate_server_base_url, McpRuntimeConfig, McpSharedState, McpTransportMode,
};
use digest_mcp::desktop::NoopDesktopViewController;
use digest_mcp::tools::build_default_dispatcher;
use digest_mcp::transport::http_bridge::spawn_http_bridge;
use digest_mcp::transport::stdio::serve_stdio;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    init_stderr_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }

    match run_cli(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{SERVICE_NAME} MCP error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn init_stderr_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

fn print_usage() {
    eprintln!(
        "{SERVICE_NAME} MCP server\n\
         Usage: digest-mcp [--transport stdio|http] [--port <1..65535>] [--mode disabled|read_only|read_write] [--server-url <url>]"
    );
}

fn apply_cli_args(config: &mut McpRuntimeConfig, args: &[String]) -> Result<(), CoreError> {
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        let (flag, inline_val) = match arg.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (arg.as_str(), None),
        };

        let next_value = |i: &mut usize| -> Result<String, CoreError> {
            if let Some(v) = inline_val {
                return Ok(v.to_owned());
            }
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| CoreError::InvalidInput(format!("missing value for {flag}")))
        };

        match flag {
            "--transport" => {
                let raw = next_value(&mut idx)?;
                config.transport = McpTransportMode::from_str(&raw)?;
            }
            "--port" => {
                let raw = next_value(&mut idx)?;
                let port: u16 = raw.trim().parse().map_err(|_| {
                    CoreError::InvalidInput(
                        "port must be an integer between 1 and 65535".to_owned(),
                    )
                })?;
                if port == 0 {
                    return Err(CoreError::InvalidInput(
                        "port must be between 1 and 65535".to_owned(),
                    ));
                }
                config.http_port = port;
            }
            "--mode" => {
                let raw = next_value(&mut idx)?;
                config.access_mode = McpAccessMode::from_str(&raw)?;
            }
            "--server-url" => {
                let raw = next_value(&mut idx)?;
                config.server_base_url = validate_server_base_url(&raw)?;
            }
            other => {
                return Err(CoreError::InvalidInput(format!(
                    "unknown CLI argument: {other}"
                )));
            }
        }
        idx += 1;
    }

    config.validate()
}

async fn run_cli(args: &[String]) -> Result<(), CoreError> {
    let mut config = McpRuntimeConfig::from_env()?;
    apply_cli_args(&mut config, args)?;

    let shared_state = McpSharedState::new(&config)?;
    let client = Arc::new(DigestApiClient::from_shared_state(shared_state.clone())?);
    let desktop = Arc::new(NoopDesktopViewController);
    let dispatcher = Arc::new(build_default_dispatcher(client, desktop.clone()));

    match config.transport {
        McpTransportMode::Stdio => {
            tracing::info!(
                mode = %config.access_mode,
                server_url = %config.server_base_url,
                "starting {SERVICE_NAME} MCP stdio server"
            );
            tokio::select! {
                res = serve_stdio(dispatcher, shared_state) => res,
                sig = tokio::signal::ctrl_c() => {
                    sig.map_err(|err| CoreError::Internal(format!("signal listener error: {err}")))?;
                    Ok(())
                }
            }
        }
        McpTransportMode::Http => {
            let handle =
                spawn_http_bridge(config.http_port, dispatcher, shared_state, desktop).await?;
            tracing::info!(
                addr = %handle.addr(),
                mode = %config.access_mode,
                server_url = %config.server_base_url,
                "started {SERVICE_NAME} local MCP HTTP bridge"
            );
            tokio::signal::ctrl_c()
                .await
                .map_err(|err| CoreError::Internal(format!("signal listener error: {err}")))?;
            handle.shutdown().await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_flags_override_runtime_config() {
        let mut cfg = McpRuntimeConfig::default();
        let args = vec![
            "--transport".to_owned(),
            "http".to_owned(),
            "--port=9123".to_owned(),
            "--mode".to_owned(),
            "read_write".to_owned(),
            "--server-url".to_owned(),
            "https://digest.example.com/".to_owned(),
        ];
        apply_cli_args(&mut cfg, &args).unwrap();
        assert_eq!(cfg.transport, McpTransportMode::Http);
        assert_eq!(cfg.http_port, 9123);
        assert_eq!(cfg.access_mode, McpAccessMode::ReadWrite);
        assert_eq!(cfg.server_base_url, "https://digest.example.com");
    }

    #[test]
    fn cli_rejects_unknown_flags_and_zero_port() {
        let mut cfg = McpRuntimeConfig::default();
        assert!(apply_cli_args(&mut cfg, &["--unknown".to_owned()]).is_err());
        assert!(apply_cli_args(&mut cfg, &["--port".to_owned(), "0".to_owned()]).is_err());
    }
}
