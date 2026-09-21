pub mod action_items;
pub mod auth;
pub mod config;
pub mod error;
pub mod facilitator;
pub mod memory_engine_client;
pub mod protocol;
pub mod rooms;
pub mod store;
pub mod ws;

pub const PRODUCT_NAME: &str = "ThoughtKhoral";
pub const SERVICE_NAME: &str = "thought-khoral-room-gateway";

pub use auth::{Actor, ActorRole, AuthConfigurationError, AuthValidator};
pub use facilitator::{
    MemoryEngineDraftProposal, MemoryProposalValidationError, NewDecisionProposal,
    validate_memory_engine_proposal,
};
pub use protocol::{
    ChatDelivery, ChatMention, ChatMentionAlias, ChatSend, MAX_CHAT_MENTIONS, RpcError,
    ValidatedRequest, validate_request,
};
pub use rooms::{GatewayState, WebSocketPolicy, WebSocketPolicyError};
pub use store::{
    AGENT_TASK_LEASE_DURATION, AgentSkillId, AgentTaskContext, AgentTaskLease, AgentTaskRecord,
    AgentTaskStart, AgentTaskStartResult, AgentTaskState, AgentTaskStore, AgentTaskUpdate,
    NewEvent, RoomEvent, StoreError, agent_task, append_event, claim_agent_task, context_for_lease,
    events_after, record_agent_task_update,
};
pub use ws::{app, gateway_status};
