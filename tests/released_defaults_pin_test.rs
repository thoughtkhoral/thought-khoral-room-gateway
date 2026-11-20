use serde_json::{Value, json};

const RELEASE_LOCK_SHA256: &str =
    "b7f3ad928c5c17be7e936da4959b7e7e8555bb4ed3602fa62f71a8721f042d8f";

fn verify_release(root: &std::path::Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    if !std::fs::symlink_metadata(root)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("release root is not a regular directory".into());
    }
    fn files(
        root: &std::path::Path,
        at: &std::path::Path,
        set: &mut BTreeSet<String>,
        directories: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(at).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                directories.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
                files(root, &path, set, directories)?;
            } else if kind.is_file() {
                set.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
            } else {
                return Err("non-regular release entry".into());
            }
        }
        Ok(())
    }
    let mut actual = BTreeSet::new();
    let mut actual_directories = BTreeSet::new();
    files(root, root, &mut actual, &mut actual_directories)?;
    let bytes = std::fs::read(root.join("lock.json")).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != RELEASE_LOCK_SHA256 {
        return Err("release lock digest".into());
    }
    let mut lock: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let payload = lock.as_object_mut().unwrap().remove("files").unwrap();
    if lock
        != json!({
                    "profile": "thought-khoral.agent-conversation.v1",
                    "repository": "https://github.com/thoughtkhoral/thought-khoral-contracts",
                    "tag": "thought-khoral-agent-conversation-v1.1.0",
                    "archiveFile": "thought-khoral-agent-conversation-v1.1.0.tar",
                    "archivePrefix": "thought-khoral-contracts/",
                    "archiveUrl": "https://github.com/thoughtkhoral/thought-khoral-contracts/releases/download/thought-khoral-agent-conversation-v1.1.0/thought-khoral-agent-conversation-v1.1.0.tar",
                    "commit": "f1c8621810c8c121c1aafc8e5a342d6626c4b843",
                    "archiveSha256": "b0370e43627ce6044852df75bfbc4b749d904aa9cedcfc558c61bb4e07dc95ff"
        })
    {
        return Err("release provenance".into());
    }
    let mut expected = BTreeSet::from(["lock.json".to_owned()]);
    let mut expected_directories = BTreeSet::new();
    for (path, digest) in payload.as_object().unwrap() {
        expected.insert(path.clone());
        let mut parent = std::path::Path::new(path).parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(directory.to_str().unwrap().to_owned());
            parent = directory.parent();
        }
        let bytes = std::fs::read(root.join(path)).map_err(|e| e.to_string())?;
        if format!("{:x}", Sha256::digest(&bytes)) != digest.as_str().unwrap() {
            return Err(format!("release payload digest: {path}"));
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
    if actual != expected || actual_directories != expected_directories {
        return Err("release file set".into());
    }
    Ok(())
}

#[test]
fn release_has_exact_closed_provenance_files_and_published_bytes() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/agent-conversation-v1.1");
    verify_release(&root).unwrap();
}

#[test]
fn release_rejects_payload_lock_extra_file_and_symlink_tampering() {
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
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/agent-conversation-v1.1");
    let target = std::env::temp_dir().join(format!("candidate-pin-{}", uuid::Uuid::new_v4()));
    copy(&source, &target);
    verify_release(&target).unwrap();
    for path in [
        "lock.json",
        "schemas/agent-conversation-v1/resolved-settings.schema.json",
    ] {
        let file = target.join(path);
        let original = std::fs::read(&file).unwrap();
        std::fs::write(&file, b"{}").unwrap();
        assert!(verify_release(&target).is_err(), "{path}");
        std::fs::write(file, original).unwrap();
    }
    let extra = target.join("extra.json");
    std::fs::write(&extra, b"{}").unwrap();
    assert!(verify_release(&target).is_err());
    std::fs::remove_file(&extra).unwrap();
    std::fs::create_dir(&extra).unwrap();
    assert!(
        verify_release(&target).is_err(),
        "unexpected empty directory"
    );
    std::fs::remove_dir(&extra).unwrap();
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
        assert!(verify_release(&target).is_err());
        std::fs::remove_file(&file).unwrap();
        std::fs::write(file, original).unwrap();
    }
    verify_release(&target).unwrap();
    std::fs::remove_dir_all(target).unwrap();
}
