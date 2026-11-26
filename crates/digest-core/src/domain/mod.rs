//! Strongly typed domain models and protocol enumerations.

pub mod enums;

pub use enums::{
    AlertPriority, AuthMode, DeliveryStatus, ErrorCode, FetchOutcome, McpAccessMode, McpToolName,
    NotificationChannelType, RunStatus, RunType, SourceScope, SourceType, SseEventType,
};
