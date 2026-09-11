#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub database_url: String,
}

impl GatewayConfig {
    pub fn from_database_url(database_url: impl Into<String>) -> Self {
        Self {
            database_url: database_url.into(),
        }
    }
}
