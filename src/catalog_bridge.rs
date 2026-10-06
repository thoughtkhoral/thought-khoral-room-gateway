//! Authenticated catalog mediation through the single admitted gateway origin.
use crate::{
    conversation_protocol::{CODEX_AGENT_ID, ConversationError, validate_profile_value},
    conversation_service::{CatalogQuery, parse_strict_json},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Mutex, time::Duration};
use uuid::Uuid;
const ENDPOINT: &str =
    "http://thought-khoral-agent-gateway:9092/internal/agent-conversations/v1/models";
const MAX_CATALOG_BYTES: usize = 1_048_576;

/// Opaque, never serializable or printable bridge credential.
#[derive(Clone)]
pub struct CatalogBridgeSecret(String);
impl std::fmt::Debug for CatalogBridgeSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CatalogBridgeSecret([redacted])")
    }
}
impl CatalogBridgeSecret {
    pub fn new(value: String) -> Result<Self, ConversationError> {
        if !(32..=4096).contains(&value.len())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err(ConversationError::RuntimeUnavailable);
        }
        Ok(Self(value))
    }
}
pub struct CatalogBridge {
    cursors: Mutex<HashMap<String, ([u8; 32], usize)>>,
    client: reqwest::Client,
    authorization: reqwest::header::HeaderValue,
}
impl CatalogBridge {
    pub fn new(secret: CatalogBridgeSecret) -> Result<Self, ConversationError> {
        Self::build(secret, None)
    }
    #[cfg(test)]
    fn with_loopback_dns(
        secret: CatalogBridgeSecret,
        address: std::net::SocketAddr,
    ) -> Result<Self, ConversationError> {
        if !address.ip().is_loopback() {
            return Err(ConversationError::RuntimeUnavailable);
        }
        Self::build(secret, Some(address))
    }
    fn build(
        secret: CatalogBridgeSecret,
        address: Option<std::net::SocketAddr>,
    ) -> Result<Self, ConversationError> {
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", secret.0))
                .map_err(|_| ConversationError::RuntimeUnavailable)?;
        authorization.set_sensitive(true);
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5));
        if let Some(address) = address {
            builder = builder.resolve("thought-khoral-agent-gateway", address);
        }
        let client = builder
            .build()
            .map_err(|_| ConversationError::RuntimeUnavailable)?;
        Ok(Self {
            cursors: Mutex::new(HashMap::new()),
            client,
            authorization,
        })
    }
}
impl CatalogQuery for CatalogBridge {
    fn page<'a>(
        &'a self,
        agent: Uuid,
        cursor: Option<String>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ConversationError>> + Send + 'a>> {
        Box::pin(async move {
            if agent != CODEX_AGENT_ID || !(1..=100).contains(&limit) {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let resume = if let Some(cursor) = cursor {
                Some(
                    *self
                        .cursors
                        .lock()
                        .map_err(|_| ConversationError::RuntimeUnavailable)?
                        .get(&cursor)
                        .ok_or(ConversationError::RuntimeUnavailable)?,
                )
            } else {
                None
            };
            let mut response = self
                .client
                .get(ENDPOINT)
                .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
                .send()
                .await
                .map_err(|_| ConversationError::RuntimeUnavailable)?;
            if response.status() != reqwest::StatusCode::OK
                || response
                    .content_length()
                    .is_some_and(|n| n > MAX_CATALOG_BYTES as u64)
            {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| ConversationError::RuntimeUnavailable)?
            {
                if chunk.len() > MAX_CATALOG_BYTES - bytes.len() {
                    return Err(ConversationError::RuntimeUnavailable);
                }
                bytes.extend_from_slice(&chunk);
            }
            let mut page =
                parse_strict_json(&bytes).map_err(|_| ConversationError::RuntimeUnavailable)?;
            validate_profile_value("catalog", &page)
                .map_err(|_| ConversationError::RuntimeUnavailable)?;
            if !page["nextCursor"].is_null() {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let digest: [u8; 32] = Sha256::digest(page.to_string().as_bytes()).into();
            let offset = if let Some((bound, offset)) = resume {
                if bound != digest {
                    return Err(ConversationError::ConversationStale);
                }
                offset
            } else {
                0
            };
            let data = page["data"]
                .as_array_mut()
                .ok_or(ConversationError::RuntimeUnavailable)?;
            if offset > data.len() {
                return Err(ConversationError::RuntimeUnavailable);
            }
            let end = (offset + limit).min(data.len());
            let has_next = end < data.len();
            *data = data[offset..end].to_vec();
            if has_next {
                let cursor = Uuid::new_v4().to_string();
                let mut cursors = self
                    .cursors
                    .lock()
                    .map_err(|_| ConversationError::RuntimeUnavailable)?;
                if cursors.len() >= 1024
                    && let Some(old) = cursors.keys().next().cloned()
                {
                    cursors.remove(&old);
                }
                cursors.insert(cursor.clone(), (digest, end));
                page["nextCursor"] = Value::String(cursor);
            }
            Ok(page)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_protocol::{CODEX_AGENT_ID, PROFILE_VERSION};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const SECRET: &str = "catalog-bridge-distinct-test-secret";
    static FIXTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    async fn fixture(
        status: &str,
        body: String,
    ) -> (CatalogBridge, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:9092")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:1/private\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 4096];
            let n = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8(bytes[..n].to_vec()).unwrap();
            let _ = stream.write_all(response.as_bytes()).await;
            request
        });
        (
            CatalogBridge::with_loopback_dns(
                CatalogBridgeSecret::new(SECRET.into()).unwrap(),
                address,
            )
            .unwrap(),
            task,
        )
    }
    fn page() -> String {
        serde_json::json!({"profileVersion":PROFILE_VERSION,"catalogRevision":"c","data":[],"nextCursor":null}).to_string()
    }
    #[tokio::test]
    async fn fixed_authenticated_get_returns_full_page() {
        let _guard = FIXTURE_LOCK.lock().await;
        let (bridge, task) = fixture("200 OK", page()).await;
        assert_eq!(
            bridge.page(CODEX_AGENT_ID, None, 100).await.unwrap()["catalogRevision"],
            "c"
        );
        let request = task.await.unwrap();
        assert!(request.starts_with("GET /internal/agent-conversations/v1/models HTTP/1.1\r\n"));
        assert!(request.contains(&format!("authorization: Bearer {SECRET}\r\n")));
        assert!(request.contains("host: thought-khoral-agent-gateway:9092\r\n"));
        assert!(request.ends_with("\r\n\r\n"));
    }
    #[tokio::test]
    async fn rejects_redirect_malformed_private_duplicate_fractional_and_oversized_pages() {
        let _guard = FIXTURE_LOCK.lock().await;
        let mut private: Value = serde_json::from_str(&page()).unwrap();
        private["nativeThreadId"] = Value::String("private".into());
        let mut cursor: Value = serde_json::from_str(&page()).unwrap();
        cursor["nextCursor"] = Value::String("next".into());
        for (status, body) in [
            ("302 Found", page()),
            ("401 Unauthorized", page()),
            ("200 OK", "{}".into()),
            ("200 OK", private.to_string()),
            ("200 OK", cursor.to_string()),
            (
                "200 OK",
                page().replacen("{", "{\"catalogRevision\":\"other\",", 1),
            ),
            ("200 OK", page().replace("null", "1.0")),
            ("200 OK", " ".repeat(MAX_CATALOG_BYTES + 1)),
        ] {
            let (bridge, task) = fixture(status, body).await;
            assert!(matches!(
                bridge.page(CODEX_AGENT_ID, None, 100).await,
                Err(ConversationError::RuntimeUnavailable)
            ));
            task.await.unwrap();
        }
    }
    #[tokio::test]
    async fn locally_paginates_full_catalog_and_invalidates_changed_content() {
        let _guard = FIXTURE_LOCK.lock().await;
        let mut full: Value = serde_json::from_str(&page()).unwrap();
        full["data"] = serde_json::json!([
            {"id":"m1","displayName":"One","defaultReasoningEffort":null,"supportedReasoningEfforts":[]},
            {"id":"m2","displayName":"Two","defaultReasoningEffort":null,"supportedReasoningEfforts":[]}
        ]);
        let (bridge, task) = fixture("200 OK", full.to_string()).await;
        let first = bridge.page(CODEX_AGENT_ID, None, 1).await.unwrap();
        task.await.unwrap();
        assert_eq!(first["data"].as_array().unwrap().len(), 1);
        let cursor = first["nextCursor"].as_str().unwrap().to_owned();
        let (_, task) = fixture("200 OK", full.to_string()).await;
        let second = bridge
            .page(CODEX_AGENT_ID, Some(cursor.clone()), 1)
            .await
            .unwrap();
        task.await.unwrap();
        assert_eq!(second["data"][0]["id"], "m2");
        assert!(second["nextCursor"].is_null());
        full["data"][1]["displayName"] = Value::String("Changed".into());
        let (_, task) = fixture("200 OK", full.to_string()).await;
        assert!(bridge.page(CODEX_AGENT_ID, Some(cursor), 1).await.is_err());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn bounds_streamed_body_and_total_read_deadline() {
        let _guard = FIXTURE_LOCK.lock().await;
        for slow in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:9092")
                .await
                .unwrap();
            let bridge = CatalogBridge::with_loopback_dns(
                CatalogBridgeSecret::new(SECRET.into()).unwrap(),
                listener.local_addr().unwrap(),
            )
            .unwrap();
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                assert!(stream.read(&mut bytes).await.unwrap() > 0);
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                if slow {
                    tokio::time::sleep(Duration::from_secs(6)).await;
                } else {
                    let _ = stream.write_all(&vec![b' '; MAX_CATALOG_BYTES + 1]).await;
                }
            });
            assert!(matches!(
                bridge.page(CODEX_AGENT_ID, None, 100).await,
                Err(ConversationError::RuntimeUnavailable)
            ));
            task.abort();
        }
    }

    #[tokio::test]
    async fn rejects_invalid_requests_before_network() {
        let bridge = CatalogBridge::new(CatalogBridgeSecret::new(SECRET.into()).unwrap()).unwrap();
        for (agent, cursor, limit) in [
            (Uuid::nil(), None, 100),
            (CODEX_AGENT_ID, Some("x".into()), 100),
            (CODEX_AGENT_ID, None, 0),
            (CODEX_AGENT_ID, None, 101),
        ] {
            assert!(bridge.page(agent, cursor, limit).await.is_err());
        }
    }
}
