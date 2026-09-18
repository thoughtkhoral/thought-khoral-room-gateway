pub mod auth;
pub mod config;
pub mod error;
pub mod facilitator;
pub mod protocol;
pub mod rooms;
pub mod store;
pub mod ws;

pub const PRODUCT_NAME: &str = "ThoughtKhoral";
pub const SERVICE_NAME: &str = "thought-khoral-room-gateway";

pub use auth::{Actor, ActorRole, AuthConfigurationError, AuthValidator};
pub use facilitator::{
    MemoryEngineDraftProposal, MemoryProposalValidationError, NewDecisionProposal,
    propose_from_message, validate_memory_engine_proposal,
};
pub use protocol::{ChatSend, RpcError, ValidatedRequest, validate_request};
pub use rooms::{GatewayState, WebSocketPolicy, WebSocketPolicyError};
pub use store::{NewEvent, RoomEvent, StoreError, append_event, events_after};
pub use ws::{app, gateway_status};
