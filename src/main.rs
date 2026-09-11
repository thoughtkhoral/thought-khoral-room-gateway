use n2n_room_gateway::{AuthValidator, GatewayState, app, config::GatewayConfig};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();
    let config = GatewayConfig::from_env()?;
    let auth = AuthValidator::new(config.oidc_issuer, config.oidc_audience, &config.oidc_jwks)?;
    let pool = PgPoolOptions::new().connect(&config.database_url).await?;
    let listener = tokio::net::TcpListener::bind(config.listen_address).await?;
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
