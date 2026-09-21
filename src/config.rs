use std::{net::SocketAddr, time::Duration};

use crate::{WebSocketPolicy, memory_engine_client::MemoryEngineClientConfig};

/// The sole Keycloak client allowed to use the HTTP-only internal agent surface.
pub const AGENT_GATEWAY_CLIENT_ID: &str = "thought-khoral-agent-gateway";
/// Internal workloads authenticate to the gateway service itself, never to the workspace UI.
pub const AGENT_GATEWAY_AUDIENCE: &str = "thought-khoral-room-gateway";

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub database_url: String,
    pub listen_address: SocketAddr,
    pub oidc_issuer: String,
    pub oidc_audience: String,
    pub oidc_jwks: String,
    pub websocket_policy: WebSocketPolicy,
    pub memory_engine: Option<MemoryEngineClientConfig>,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let allowed_origins = required(&lookup, "THOUGHT_KHORAL_ALLOWED_ORIGINS")?
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let authentication_timeout = lookup("THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS")
            .unwrap_or_else(|| "5000".to_owned())
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| ConfigError("THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS"))?;
        let websocket_policy = WebSocketPolicy::new(allowed_origins, authentication_timeout)
            .map_err(|_| {
                ConfigError(
                    "THOUGHT_KHORAL_ALLOWED_ORIGINS or THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS",
                )
            })?;

        let memory_engine = match (
            lookup("THOUGHT_KHORAL_MEMORY_ENGINE_URL"),
            lookup("THOUGHT_KHORAL_MEMORY_ENGINE_SHARED_SECRET"),
        ) {
            (None, None) => None,
            (Some(endpoint), Some(shared_secret))
                if !endpoint.is_empty() && !shared_secret.is_empty() =>
            {
                let timeout = lookup("THOUGHT_KHORAL_MEMORY_ENGINE_TIMEOUT_MS")
                    .unwrap_or_else(|| "100".to_owned())
                    .parse::<u64>()
                    .map(Duration::from_millis)
                    .map_err(|_| ConfigError("THOUGHT_KHORAL_MEMORY_ENGINE_TIMEOUT_MS"))?;
                let queue_capacity = lookup("THOUGHT_KHORAL_MEMORY_ENGINE_QUEUE_CAPACITY")
                    .unwrap_or_else(|| "64".to_owned())
                    .parse::<usize>()
                    .map_err(|_| ConfigError("THOUGHT_KHORAL_MEMORY_ENGINE_QUEUE_CAPACITY"))?;
                Some(MemoryEngineClientConfig {
                    endpoint,
                    shared_secret,
                    timeout,
                    queue_capacity,
                })
            }
            _ => {
                return Err(ConfigError(
                    "THOUGHT_KHORAL_MEMORY_ENGINE_URL and THOUGHT_KHORAL_MEMORY_ENGINE_SHARED_SECRET",
                ));
            }
        };

        Ok(Self {
            database_url: required(&lookup, "DATABASE_URL")?,
            listen_address: lookup("THOUGHT_KHORAL_LISTEN_ADDRESS")
                .unwrap_or_else(|| "127.0.0.1:8080".to_owned())
                .parse()
                .map_err(|_| ConfigError("THOUGHT_KHORAL_LISTEN_ADDRESS"))?,
            oidc_issuer: required(&lookup, "THOUGHT_KHORAL_OIDC_ISSUER")?,
            oidc_audience: required(&lookup, "THOUGHT_KHORAL_OIDC_AUDIENCE")?,
            oidc_jwks: required(&lookup, "THOUGHT_KHORAL_OIDC_JWKS")?,
            websocket_policy,
            memory_engine,
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

fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    lookup(name)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError(name))
}

#[cfg(test)]
mod tests {
    use super::GatewayConfig;

    // This fails if the gateway stops consuming the ThoughtKhoral configuration namespace.
    #[test]
    fn reads_thought_khoral_configuration_names() {
        let config = GatewayConfig::from_lookup(|name| {
            match name {
                "DATABASE_URL" => Some("postgres://database.test/n2n"),
                "THOUGHT_KHORAL_ALLOWED_ORIGINS" => Some("https://workspace.test"),
                "THOUGHT_KHORAL_LISTEN_ADDRESS" => Some("127.0.0.1:9090"),
                "THOUGHT_KHORAL_OIDC_ISSUER" => Some("https://identity.test/realms/thought-khoral"),
                "THOUGHT_KHORAL_OIDC_AUDIENCE" => Some("thought-khoral-room-gateway"),
                "THOUGHT_KHORAL_OIDC_JWKS" => Some(r#"{"keys":[]}"#),
                "THOUGHT_KHORAL_SESSION_AUTH_TIMEOUT_MS" => Some("2500"),
                _ => None,
            }
            .map(str::to_owned)
        })
        .expect("the canonical configuration must parse");

        assert_eq!(config.listen_address.to_string(), "127.0.0.1:9090");
        assert_eq!(
            config.oidc_issuer,
            "https://identity.test/realms/thought-khoral"
        );
        assert_eq!(config.oidc_audience, "thought-khoral-room-gateway");
        assert_eq!(config.oidc_jwks, r#"{"keys":[]}"#);
        assert_eq!(
            config.websocket_policy.authentication_timeout().as_millis(),
            2500
        );
        assert!(
            config
                .websocket_policy
                .allows_origin("https://workspace.test")
        );
    }

    // This fails if a stale deployment can start through an implicit legacy alias.
    #[test]
    fn rejects_legacy_configuration_names() {
        let error = GatewayConfig::from_lookup(|name| {
            match name {
                "DATABASE_URL" => Some("postgres://database.test/n2n"),
                "N2N_ALLOWED_ORIGINS" => Some("https://workspace.test"),
                "N2N_OIDC_ISSUER" => Some("https://identity.test/realms/legacy"),
                "N2N_OIDC_AUDIENCE" => Some("legacy-room-gateway"),
                "N2N_OIDC_JWKS" => Some(r#"{"keys":[]}"#),
                _ => None,
            }
            .map(str::to_owned)
        })
        .expect_err("legacy configuration must not act as an alias");

        assert_eq!(
            error.to_string(),
            "missing or invalid required configuration: THOUGHT_KHORAL_ALLOWED_ORIGINS"
        );
    }
}
