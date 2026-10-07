use sqlx::postgres::PgPoolOptions;
use thought_khoral_room_gateway::{
    AuthValidator, GatewayState, PRODUCT_NAME, SERVICE_NAME, app, config::GatewayConfig,
    memory_engine_client::MemoryEngineClient,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();
    let config = GatewayConfig::from_env()?;
    let auth = AuthValidator::new(config.oidc_issuer, config.oidc_audience, &config.oidc_jwks)?;
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    let listener = tokio::net::TcpListener::bind(config.listen_address).await?;
    tracing::info!(
        product = PRODUCT_NAME,
        service = SERVICE_NAME,
        listen_address = %listener.local_addr()?,
        "ThoughtKhoral room gateway listening"
    );
    let state = match config.memory_engine {
        Some(memory_engine) => GatewayState::with_memory_engine_client(
            pool,
            auth,
            config.websocket_policy,
            MemoryEngineClient::new(memory_engine),
        ),
        None => GatewayState::with_websocket_policy(pool, auth, config.websocket_policy),
    };
    let catalog = config
        .catalog_bridge_secret
        .map(|secret| {
            thought_khoral_room_gateway::catalog_bridge::CatalogBridge::new(secret).map(|bridge| {
                std::sync::Arc::new(bridge)
                    as std::sync::Arc<
                        dyn thought_khoral_room_gateway::conversation_service::CatalogQuery,
                    >
            })
        })
        .transpose()?;
    state
        .configure_conversations(config.conversation_policy, catalog)
        .await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
