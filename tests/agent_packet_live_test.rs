mod support;

use chrono::Utc;
use serde_json::{Value, json};
use support::{AUDIENCE, TestServer};
use thought_khoral_room_gateway::{AgentSkillId, AgentTaskStart, AgentTaskStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

// Run with the reference-agent binary on loopback using the explicit test
// secret below. This exercises the real broker packet over the A2A server;
// there is no hand-written packet fixture or sibling crate dependency.
#[tokio::test]
#[ignore = "requires the local reference-agent binary on 127.0.0.1:9090"]
async fn actual_broker_packets_execute_both_skills_over_a2a() {
    let server = TestServer::start().await;
    let requester = Uuid::new_v4();
    let agent = Uuid::from_u128(0x74686f756768746b_686f72616c000003);
    let store = AgentTaskStore::new(server.pool.clone(), requester, "Packet test human");
    let token = server.agent_gateway_token(AUDIENCE, "thought-khoral-agent-gateway");
    for skill in [
        AgentSkillId::SummarizeContext,
        AgentSkillId::ExtractActionItems,
    ] {
        let task = store
            .start_agent_task(AgentTaskStart {
                id: "live-packet".to_owned(),
                contract_version: "n2n.room.v1".to_owned(),
                request_id: Uuid::new_v4(),
                room_id: Uuid::new_v4(),
                occurred_at: Utc::now(),
                agent_id: agent,
                skill_id: skill,
                input: "- Review the broker packet | owner: Maya | due: Friday".to_owned(),
            })
            .await
            .unwrap();
        let lease = store
            .claim_agent_task(task.task_id, Uuid::new_v4(), Utc::now())
            .await
            .unwrap()
            .unwrap();
        let (status, context) = server
            .internal_json(
                "GET",
                &format!("/internal/v1/agent-tasks/{}/context", task.task_id),
                Some(&token),
                Some(lease.owner_id),
                None,
            )
            .await;
        assert_eq!(status, 200);
        let packet = &context["packet"];
        let invocation = packet["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["eventType"] == "agent.task.requested")
            .unwrap();
        assert!(invocation["payload"].get("input").is_none());
        let body = json!({"jsonrpc":"2.0", "id": 1, "method":"SendStreamingMessage", "params": {
            "message": { "messageId": Uuid::new_v4(), "taskId": task.task_id,
                "contextId": format!("{}:{}", packet["roomId"].as_str().unwrap(), packet["contextRevision"]),
                "role": "ROLE_USER", "parts": [{"text": json!({"packet": packet}).to_string()}] }
        }}).to_string();
        let mut socket = tokio::net::TcpStream::connect("127.0.0.1:9090")
            .await
            .unwrap();
        socket.write_all(format!("POST /jsonrpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer packet-integration-test-only\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.read_to_string(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let events: Vec<Value> = response
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|value| serde_json::from_str(value.trim()).unwrap())
            .collect();
        assert_eq!(events.len(), 4, "{response}");
        assert!(response.contains("TASK_STATE_COMPLETED"), "{response}");
        assert!(response.contains(invocation["eventId"].as_str().unwrap()));
        if skill == AgentSkillId::ExtractActionItems {
            assert!(response.contains("Review the broker packet"));
        }
    }
}
