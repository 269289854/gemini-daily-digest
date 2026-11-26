//! Core domain types, zero-magic-value constants, typed configuration,
//! structured errors, cache coordination SPI, and extension traits.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cache;
pub mod collectors;
pub mod config;
pub mod constants;
pub mod crypto;
pub mod domain;
pub mod error;
pub mod http;
pub mod llm;
pub mod notifiers;
pub mod pipeline;
pub mod presets;
pub mod renderer;
pub mod ssrf;
pub mod traits;
pub mod workflow;
