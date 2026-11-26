//! Core domain types, zero-magic-value constants, typed configuration,
//! structured errors, cache coordination SPI, and extension traits.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cache;
pub mod config;
pub mod constants;
pub mod domain;
pub mod error;
pub mod traits;
