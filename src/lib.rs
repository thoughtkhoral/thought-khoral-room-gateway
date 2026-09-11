pub mod auth;
pub mod config;
pub mod error;
pub mod protocol;
pub mod rooms;
pub mod store;
pub mod ws;

pub use auth::{Actor, ActorRole, AuthConfigurationError, AuthValidator};
pub use protocol::{ChatSend, RpcError, ValidatedRequest, validate_request};
pub use rooms::{GatewayState, WebSocketPolicy, WebSocketPolicyError};
pub use store::{NewEvent, RoomEvent, StoreError, append_event, events_after};
pub use ws::app;
