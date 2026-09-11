use std::{net::SocketAddr, time::Duration};

use crate::WebSocketPolicy;

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub database_url: String,
    pub listen_address: SocketAddr,
    pub oidc_issuer: String,
    pub oidc_audience: String,
    pub oidc_jwks: String,
    pub websocket_policy: WebSocketPolicy,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let allowed_origins = required("N2N_ALLOWED_ORIGINS")?
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let authentication_timeout = std::env::var("N2N_SESSION_AUTH_TIMEOUT_MS")
            .unwrap_or_else(|_| "5000".to_owned())
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| ConfigError("N2N_SESSION_AUTH_TIMEOUT_MS"))?;
        let websocket_policy = WebSocketPolicy::new(allowed_origins, authentication_timeout)
            .map_err(|_| ConfigError("N2N_ALLOWED_ORIGINS or N2N_SESSION_AUTH_TIMEOUT_MS"))?;

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            listen_address: std::env::var("N2N_LISTEN_ADDRESS")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
                .parse()
                .map_err(|_| ConfigError("N2N_LISTEN_ADDRESS"))?,
            oidc_issuer: required("N2N_OIDC_ISSUER")?,
            oidc_audience: required("N2N_OIDC_AUDIENCE")?,
            oidc_jwks: required("N2N_OIDC_JWKS")?,
            websocket_policy,
        })
    }
}

#[derive(Debug)]
pub struct ConfigError(&'static str);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "missing or invalid required configuration: {}",
            self.0
        )
    }
}

impl std::error::Error for ConfigError {}

fn required(name: &'static str) -> Result<String, ConfigError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError(name))
}
