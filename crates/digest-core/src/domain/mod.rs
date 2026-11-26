//! Strongly typed domain models and protocol enumerations.

pub mod enums;
pub mod models;

pub use enums::{
    AlertPriority, AuthMode, DeliveryStatus, ErrorCode, FetchOutcome, McpAccessMode, McpToolName,
    NotificationChannelType, RunStatus, RunType, SourceScope, SourceType, SseEventType,
};
pub use models::{
    DailyDigestSynthesisInput, DeliveryOutcome, DigestSection, DigestWorkflowInput,
    DigestWorkflowOutput, ItemMatchResult, NotifierConfig, PresetSourceSpec, ProcessedItem,
    RenderInput, SourceDigestInput, SourceFetchRecord, SourceRunInput, SourceSummaryResult,
    UserAiConfig, WatchlistRuleInput,
};
