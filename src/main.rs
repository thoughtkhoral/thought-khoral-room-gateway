use sqlx::postgres::PgPoolOptions;
use thought_khoral_room_gateway::{
    AuthValidator, GatewayState, PRODUCT_NAME, SERVICE_NAME, app, config::GatewayConfig,
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
    axum::serve(
        listener,
        app(GatewayState::with_websocket_policy(
            pool,
            auth,
            config.websocket_policy,
        )),
    )
    .await?;
    Ok(())
}
