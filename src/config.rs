use std::net::SocketAddr;

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub database_url: String,
    pub listen_address: SocketAddr,
    pub oidc_issuer: String,
    pub oidc_audience: String,
    pub oidc_jwks: String,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            listen_address: std::env::var("N2N_LISTEN_ADDRESS")
                .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
                .parse()
                .map_err(|_| ConfigError("N2N_LISTEN_ADDRESS"))?,
            oidc_issuer: required("N2N_OIDC_ISSUER")?,
            oidc_audience: required("N2N_OIDC_AUDIENCE")?,
            oidc_jwks: required("N2N_OIDC_JWKS")?,
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
