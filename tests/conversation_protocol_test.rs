use serde_json::{Value, json};
use thought_khoral_room_gateway::conversation_protocol::{
    ConversationError, validate_turn_request,
};

fn first_turn() -> Value {
    serde_json::from_str(include_str!(
        "../contracts/agent-conversation-v1/fixtures/valid/first-turn.json"
    ))
    .unwrap()
}

// Removing strict schema/identity checks would admit client authority or ambiguous invocation.
#[test]
fn accepts_the_published_first_turn() {
    let request = first_turn();
    assert_eq!(validate_turn_request(&request).unwrap(), request);
}

#[test]
fn rejects_client_authority_and_unknown_fields() {
    for field in [
        "delivery",
        "requesterId",
        "threadId",
        "endpoint",
        "directory",
    ] {
        let mut request = first_turn();
        request[field] = json!("client supplied");
        assert_eq!(
            validate_turn_request(&request),
            Err(ConversationError::InvalidTaskInput),
            "{field}"
        );
    }
}

#[test]
fn requires_one_canonical_direct_codex_identity() {
    let mut request = first_turn();
    request["agentId"] = json!("11111111-1111-4111-8111-111111111111");
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["mentions"][0]["token"] = json!("codex");
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    let agent_id = request["agentId"].clone();
    request["mentions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"participant", "id":agent_id, "token":"codex"}));
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["mentions"] = json!([{"type":"alias", "alias":"allagents"}]);
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn counts_unicode_scalars_and_rejects_float_generations() {
    let mut request = first_turn();
    request["text"] = json!("🦀".repeat(8000));
    assert!(validate_turn_request(&request).is_ok());
    request["text"] = json!("🦀".repeat(8001));
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["conversation"] = json!({"mode":"continue","id":request["requestId"],"generation":1.0});
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn duplicate_aliases_and_partial_settings_fail() {
    let mut request = first_turn();
    request["mentions"].as_array_mut().unwrap().extend([
        json!({"type":"alias","alias":"allhumans"}),
        json!({"type":"alias","alias":"allhumans"}),
    ]);
    assert!(validate_turn_request(&request).is_err());
    request = first_turn();
    request["settings"] = json!({"model":"model-a"});
    assert!(validate_turn_request(&request).is_err());
}

#[test]
fn pinned_stateless_turn_fixtures_match_the_rust_validator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/agent-conversation-v1/fixtures");
    let manifest: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for case in manifest {
        // Mutable broker state belongs to store tests. Raw duplicate-key checking
        // belongs to Task 3's HTTP decoder, because Value has already lost keys.
        if case["schema"] != "turn"
            || case.get("context").is_some()
            || case["reason"] == "json-duplicate"
        {
            continue;
        }
        let bytes = std::fs::read(root.join(case["path"].as_str().unwrap())).unwrap();
        let valid = serde_json::from_slice::<Value>(&bytes)
            .is_ok_and(|value| validate_turn_request(&value).is_ok());
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["path"]);
    }
}

#[test]
fn vendored_files_match_the_published_artifact_lock() {
    use sha2::{Digest, Sha256};
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/agent-conversation-v1");
    let lock: Value =
        serde_json::from_slice(&std::fs::read(root.join("lock.json")).unwrap()).unwrap();
    assert_eq!(lock["commit"], "85baf86e574276fcd036e53e23641af6aad602f9");
    assert_eq!(
        lock["archiveSha256"],
        "0038fdbf858db013c4a269aeedbca51a5db9128a2f1d6e39754f92ba60fd8f36"
    );
    for (path, digest) in lock["files"].as_object().unwrap() {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(path)).unwrap())
            ),
            digest.as_str().unwrap(),
            "{path}"
        );
    }
}

const CANDIDATE_LOCK_SHA256: &str =
    "7914d32eae2487879a68405b5095a6b9aa91355f87529c43f4055844821902a9";

fn verify_candidate(root: &std::path::Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    if !std::fs::symlink_metadata(root)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("candidate root is not a regular directory".into());
    }
    fn files(
        root: &std::path::Path,
        at: &std::path::Path,
        set: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(at).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                files(root, &path, set)?;
            } else if kind.is_file() {
                set.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            } else {
                return Err("non-regular candidate entry".into());
            }
        }
        Ok(())
    }
    let mut actual = BTreeSet::new();
    files(root, root, &mut actual)?;
    let bytes = std::fs::read(root.join("lock.json")).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != CANDIDATE_LOCK_SHA256 {
        return Err("candidate lock digest".into());
    }
    let mut lock: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let payload = lock.as_object_mut().unwrap().remove("files").unwrap();
    if lock
        != json!({
            "profile": "thought-khoral.agent-conversation.v1",
            "status": "unreleased-local-candidate",
            "proposedTag": "thought-khoral-agent-conversation-v1.1.0",
            "repository": "https://github.com/thoughtkhoral/thought-khoral-contracts",
            "archivePrefix": "thought-khoral-contracts/",
            "baseTag": "thought-khoral-agent-conversation-v1.0.0",
            "baseCommit": "85baf86e574276fcd036e53e23641af6aad602f9",
            "baseArchiveSha256": "0038fdbf858db013c4a269aeedbca51a5db9128a2f1d6e39754f92ba60fd8f36",
            "commit": "1ea828f28725ddaaefa21d083473f9abbd777975",
            "archiveFile": "thought-khoral-agent-conversation-v1.1.0-candidate-1ea828f28725ddaaefa21d083473f9abbd777975.tar",
            "archiveSha256": "fab59a486f6498b843467202debcb0768403bd57ba7dda41be2a01e5f23fdda8"
        })
    {
        return Err("candidate provenance".into());
    }
    let mut expected = BTreeSet::from(["lock.json".to_owned()]);
    for (path, digest) in payload.as_object().unwrap() {
        expected.insert(path.clone());
        let bytes = std::fs::read(root.join(path)).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != digest.as_str().unwrap() {
            return Err(format!("candidate payload digest: {path}"));
        }
    }
    let published =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/agent-conversation-v1");
    let old_lock: Value = serde_json::from_slice(
        &std::fs::read(published.join("lock.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    for path in old_lock["files"].as_object().unwrap().keys() {
        let (category, relative) = path.split_once('/').ok_or("published path")?;
        let translated = root
            .join(category)
            .join("agent-conversation-v1")
            .join(relative);
        if std::fs::read(translated).map_err(|e| e.to_string())?
            != std::fs::read(published.join(path)).map_err(|e| e.to_string())?
        {
            return Err(format!("published compatibility: {path}"));
        }
    }
    if actual != expected {
        return Err("candidate file set".into());
    }
    Ok(())
}

#[test]
fn candidate_has_exact_closed_provenance_files_and_published_bytes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/agent-conversation-v1.1-candidate");
    verify_candidate(&root).unwrap();
}

#[test]
fn candidate_rejects_payload_lock_extra_file_and_symlink_tampering() {
    fn copy(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let dest = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/agent-conversation-v1.1-candidate");
    let target = std::env::temp_dir().join(format!("candidate-pin-{}", uuid::Uuid::new_v4()));
    copy(&source, &target);
    verify_candidate(&target).unwrap();
    for path in [
        "lock.json",
        "schemas/agent-conversation-v1/resolved-settings.schema.json",
    ] {
        let file = target.join(path);
        let original = std::fs::read(&file).unwrap();
        std::fs::write(&file, b"{}").unwrap();
        assert!(verify_candidate(&target).is_err(), "{path}");
        std::fs::write(file, original).unwrap();
    }
    let extra = target.join("extra.json");
    std::fs::write(&extra, b"{}").unwrap();
    assert!(verify_candidate(&target).is_err());
    std::fs::remove_file(&extra).unwrap();
    #[cfg(unix)]
    {
        let file = target.join("schemas/agent-conversation-v1/resolved-settings.schema.json");
        let original = std::fs::read(&file).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(
            source.join("schemas/agent-conversation-v1/resolved-settings.schema.json"),
            &file,
        )
        .unwrap();
        assert!(verify_candidate(&target).is_err());
        std::fs::remove_file(&file).unwrap();
        std::fs::write(file, original).unwrap();
    }
    verify_candidate(&target).unwrap();
    std::fs::remove_dir_all(target).unwrap();
}
