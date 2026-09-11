use crate::{RpcError, StoreError};

#[derive(Debug)]
pub enum GatewayError {
    Protocol(RpcError),
    Store(StoreError),
}

impl From<RpcError> for GatewayError {
    fn from(error: RpcError) -> Self {
        Self::Protocol(error)
    }
}

impl From<StoreError> for GatewayError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
