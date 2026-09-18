#![allow(dead_code)]

use std::{net::SocketAddr, sync::LazyLock, time::Duration};

use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{
    Algorithm, EncodingKey, Header, encode,
    jwk::{Jwk, JwkSet, PublicKeyUse},
};
use rand::thread_rng;
use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use thought_khoral_room_gateway::{
    AuthValidator, GatewayState, WebSocketPolicy, app, memory_engine_client::MemoryEngineClient,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{
            HeaderValue,
            header::{AUTHORIZATION, ORIGIN},
        },
    },
};
use uuid::Uuid;

const ISSUER: &str = "http://keycloak.test/realms/thought-khoral";
const AUDIENCE: &str = "thought-khoral-room-gateway";
const KEY_ID: &str = "integration-key";
pub const BROWSER_ORIGIN: &str = "http://workspace.test";

pub type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct TestKeys {
    encoding_key: EncodingKey,
    other_encoding_key: EncodingKey,
    jwks: String,
}

static TEST_KEYS: LazyLock<TestKeys> = LazyLock::new(|| {
    let private_key =
        RsaPrivateKey::new(&mut thread_rng(), 2048).expect("the test RSA key must be generated");
    let private_der = private_key
        .to_pkcs1_der()
        .expect("the test RSA key must encode as PKCS#1");
    let encoding_key = EncodingKey::from_rsa_der(private_der.as_bytes());
    let other_private_key = RsaPrivateKey::new(&mut thread_rng(), 1024)
        .expect("the alternate test RSA key must be generated");
    let other_private_der = other_private_key
        .to_pkcs1_der()
        .expect("the alternate test RSA key must encode as PKCS#1");
    let other_encoding_key = EncodingKey::from_rsa_der(other_private_der.as_bytes());
    let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256)
        .expect("the public JWK must derive from the test key");
    jwk.common.key_id = Some(KEY_ID.to_owned());
    jwk.common.public_key_use = Some(PublicKeyUse::Signature);
    let jwks = serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap();
    TestKeys {
        encoding_key,
        other_encoding_key,
        jwks,
    }
});

pub struct TestServer {
    pub address: SocketAddr,
    pub pool: PgPool,
    pub state: GatewayState,
    encoding_key: EncodingKey,
    other_encoding_key: EncodingKey,
    task: JoinHandle<()>,
}

#[derive(Serialize)]
struct Claims {
    sub: String,
    #[serde(rename = "n2n_role")]
    role: String,
    iss: String,
    aud: String,
    exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    nbf: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preferred_username: Option<String>,
}

pub struct TokenOptions {
    pub sub: String,
    pub role: String,
    pub issuer: String,
    pub audience: String,
    pub exp: i64,
    pub nbf: Option<i64>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
    pub kid: Option<String>,
    pub algorithm: Algorithm,
    pub wrong_signature: bool,
}

impl TokenOptions {
    pub fn valid(actor_id: Uuid, role: &str) -> Self {
        Self {
            sub: actor_id.to_string(),
            role: role.to_owned(),
            issuer: ISSUER.to_owned(),
            audience: AUDIENCE.to_owned(),
            exp: chrono::Utc::now().timestamp() + 3_600,
            nbf: None,
            name: None,
            preferred_username: None,
            kid: Some(KEY_ID.to_owned()),
            algorithm: Algorithm::RS256,
            wrong_signature: false,
        }
    }
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with_authentication_timeout(Duration::from_secs(5)).await
    }

    pub async fn start_with_authentication_timeout(authentication_timeout: Duration) -> Self {
        Self::start_with_options(authentication_timeout, None).await
    }

    pub async fn start_with_memory_engine_client(client: MemoryEngineClient) -> Self {
        Self::start_with_options(Duration::from_secs(5), Some(client)).await
    }

    async fn start_with_options(
        authentication_timeout: Duration,
        memory_engine: Option<MemoryEngineClient>,
    ) -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must name a migrated PostgreSQL integration database");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("the integration database must be reachable");

        let auth = AuthValidator::new(ISSUER, AUDIENCE, &TEST_KEYS.jwks)
            .expect("the generated test JWKS must configure authentication");
        let policy = WebSocketPolicy::new([BROWSER_ORIGIN], authentication_timeout)
            .expect("the browser test policy must be valid");
        let state = match memory_engine {
            Some(memory_engine) => {
                GatewayState::with_memory_engine_client(pool.clone(), auth, policy, memory_engine)
            }
            None => GatewayState::with_websocket_policy(pool.clone(), auth, policy),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app(state)).await.unwrap();
        });

        Self {
            address,
            pool,
            state: server_state,
            encoding_key: TEST_KEYS.encoding_key.clone(),
            other_encoding_key: TEST_KEYS.other_encoding_key.clone(),
            task,
        }
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.address)
    }

    pub async fn get(&self, path: &str) -> (u16, String) {
        let address = self.address;
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};

            let mut stream = std::net::TcpStream::connect(address).unwrap();
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();

            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            let status = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|status| status.parse::<u16>().ok())
                .unwrap();
            (status, body.to_owned())
        })
        .await
        .unwrap()
    }

    pub fn token(&self, actor_id: Uuid, role: &str) -> String {
        self.token_with(TokenOptions::valid(actor_id, role))
    }

    pub fn token_with(&self, options: TokenOptions) -> String {
        let claims = Claims {
            sub: options.sub,
            role: options.role,
            iss: options.issuer,
            aud: options.audience,
            exp: options.exp,
            nbf: options.nbf,
            name: options.name,
            preferred_username: options.preferred_username,
        };
        let mut header = Header::new(options.algorithm);
        header.kid = options.kid;
        let key = if options.wrong_signature {
            &self.other_encoding_key
        } else if options.algorithm == Algorithm::HS256 {
            return encode(
                &header,
                &claims,
                &EncodingKey::from_secret(b"wrong-algorithm"),
            )
            .unwrap();
        } else {
            &self.encoding_key
        };
        encode(&header, &claims, key).unwrap()
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

    pub async fn connect_browser(&self, origin: &str) -> TestSocket {
        self.connect_browser_result(origin, None).await.unwrap().0
    }

    pub async fn connect_browser_result(
        &self,
        origin: &str,
        token: Option<&str>,
    ) -> Result<
        (
            TestSocket,
            tokio_tungstenite::tungstenite::handshake::client::Response,
        ),
        tokio_tungstenite::tungstenite::Error,
    > {
        let mut request = self.ws_url().into_client_request().unwrap();
        request
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        if let Some(token) = token {
            request.headers_mut().insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
        }
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

pub async fn recv_close_code(socket: &mut TestSocket) -> u16 {
    let message = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .expect("the gateway must close before the test timeout")
        .expect("the WebSocket close frame must be present")
        .expect("the WebSocket close frame must be valid");
    match message {
        Message::Close(Some(frame)) => frame.code.into(),
        other => panic!("expected a WebSocket close frame, got {other:?}"),
    }
}
