#![allow(dead_code)]

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{
    Algorithm, EncodingKey, Header, encode,
    jwk::{Jwk, JwkSet, PublicKeyUse},
};
use n2n_room_gateway::{AuthValidator, GatewayState, app};
use rand::thread_rng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
    },
};
use uuid::Uuid;

const ISSUER: &str = "http://keycloak.test/realms/n2n";
const AUDIENCE: &str = "n2n-room-gateway";
const KEY_ID: &str = "integration-key";

pub type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct TestServer {
    pub address: SocketAddr,
    pub pool: PgPool,
    encoding_key: EncodingKey,
    task: JoinHandle<()>,
}

#[derive(Serialize)]
struct Claims<'a> {
    sub: String,
    n2n_role: &'a str,
    iss: &'a str,
    aud: &'a str,
    exp: i64,
}

impl TestServer {
    pub async fn start() -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must name a migrated PostgreSQL integration database");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("the integration database must be reachable");

        let private_key = RsaPrivateKey::new(&mut thread_rng(), 2048)
            .expect("the test RSA key must be generated");
        let private_der = private_key
            .to_pkcs1_der()
            .expect("the test RSA key must encode as PKCS#1");
        let encoding_key = EncodingKey::from_rsa_der(private_der.as_bytes());
        let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256)
            .expect("the public JWK must derive from the test key");
        jwk.common.key_id = Some(KEY_ID.to_owned());
        jwk.common.public_key_use = Some(PublicKeyUse::Signature);
        let jwks = serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap();
        let auth = AuthValidator::new(ISSUER, AUDIENCE, &jwks)
            .expect("the generated test JWKS must configure authentication");
        let state = GatewayState::new(pool.clone(), auth);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app(state)).await.unwrap();
        });

        Self {
            address,
            pool,
            encoding_key,
            task,
        }
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    pub fn token(&self, actor_id: Uuid, role: &str) -> String {
        let claims = Claims {
            sub: actor_id.to_string(),
            n2n_role: role,
            iss: ISSUER,
            aud: AUDIENCE,
            exp: chrono::Utc::now().timestamp() + 3_600,
        };
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(KEY_ID.to_owned());
        encode(&header, &claims, &self.encoding_key).unwrap()
    }

    pub async fn connect(&self, token: &str) -> TestSocket {
        self.connect_result(token).await.unwrap().0
    }

    pub async fn connect_result(
        &self,
        token: &str,
    ) -> Result<
        (
            TestSocket,
            tokio_tungstenite::tungstenite::handshake::client::Response,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        let mut request = self.ws_url().into_client_request().unwrap();
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        connect_async(request).await
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn rpc(id: &str, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn common_params(request_id: Uuid, room_id: Uuid) -> Value {
    json!({
        "contractVersion": "n2n.room.v1",
        "requestId": request_id,
        "roomId": room_id,
        "occurredAt": chrono::Utc::now(),
    })
}

pub async fn join(socket: &mut TestSocket, room_id: Uuid, after_sequence: Option<i64>) -> Value {
    let mut params = common_params(Uuid::new_v4(), room_id);
    if let Some(after_sequence) = after_sequence {
        params["afterSequence"] = json!(after_sequence);
    }
    send_json(socket, rpc("join", "room.join", params)).await;
    recv_json(socket).await
}

pub async fn send_json(socket: &mut TestSocket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

pub async fn recv_json(socket: &mut TestSocket) -> Value {
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .expect("the gateway must respond before the test timeout")
        .expect("the WebSocket must remain open")
        .expect("the WebSocket frame must be valid");
    match message {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected a JSON text frame, got {other:?}"),
    }
}
