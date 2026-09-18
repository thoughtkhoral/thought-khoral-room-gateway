use std::{
    fmt::{Display, Formatter},
    sync::Arc,
    time::Duration,
};

use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    time::timeout,
};
use uuid::Uuid;

use crate::store::RoomEvent;

#[derive(Clone, Debug)]
pub struct MemoryEngineClientConfig {
    pub endpoint: String,
    pub shared_secret: String,
    pub timeout: Duration,
    pub queue_capacity: usize,
}

#[derive(Clone)]
pub struct MemoryEngineClient {
    queue: Arc<mpsc::Sender<DispatchJob>>,
    settings: Arc<ClientSettings>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnqueueReceipt {
    pub room_id: Uuid,
    pub event_id: Uuid,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MemoryEngineClientError {
    QueueFull,
    QueueClosed,
}

impl Display for MemoryEngineClientError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for MemoryEngineClientError {}

struct DispatchJob {
    settings: Arc<ClientSettings>,
    event: RoomEvent,
}

struct ClientSettings {
    endpoint: String,
    shared_secret: String,
    timeout: Duration,
}

impl MemoryEngineClient {
    pub fn new(config: MemoryEngineClientConfig) -> Self {
        let settings = Arc::new(ClientSettings {
            endpoint: config.endpoint,
            shared_secret: config.shared_secret,
            timeout: config.timeout,
        });
        let (sender, mut receiver) = mpsc::channel::<DispatchJob>(config.queue_capacity.max(1));
        tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                let event_id = job.event.event_id;
                let room_id = job.event.room_id;
                if let Err(error) = timeout(job.settings.timeout, send_event(job)).await {
                    tracing::warn!(%room_id, %event_id, ?error, "memory-engine dispatch timed out");
                }
            }
        });
        Self {
            queue: Arc::new(sender),
            settings,
        }
    }

    pub async fn enqueue_committed_event(
        &self,
        event: RoomEvent,
    ) -> Result<EnqueueReceipt, MemoryEngineClientError> {
        let receipt = EnqueueReceipt {
            room_id: event.room_id,
            event_id: event.event_id,
        };
        let job = DispatchJob {
            settings: Arc::clone(&self.settings),
            event,
        };
        self.queue.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => MemoryEngineClientError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => MemoryEngineClientError::QueueClosed,
        })?;
        Ok(receipt)
    }
}

async fn send_event(job: DispatchJob) -> Result<(), String> {
    let (authority, path) = parse_endpoint(&job.settings.endpoint)?;
    let mut stream = TcpStream::connect(&authority)
        .await
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_vec(&json!({
        "roomId": job.event.room_id,
        "eventId": job.event.event_id,
        "eventType": job.event.event_type,
        "occurredAt": job.event.occurred_at,
        "payload": job.event.payload,
    }))
    .map_err(|error| error.to_string())?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        job.settings.shared_secret,
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&body)
        .await
        .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|error| error.to_string())?;
    if response.starts_with(b"HTTP/1.1 2") || response.starts_with(b"HTTP/1.0 2") {
        Ok(())
    } else {
        Err("memory engine returned a non-success response".to_owned())
    }
}

fn parse_endpoint(endpoint: &str) -> Result<(String, String), String> {
    let remainder = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| "memory-engine endpoint must use http://".to_owned())?;
    let (authority, path) = remainder
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((remainder, "/internal/v1/ingest".to_owned()));
    if authority.is_empty() || path == "/" {
        return Err("memory-engine endpoint is incomplete".to_owned());
    }
    Ok((authority.to_owned(), path))
}
