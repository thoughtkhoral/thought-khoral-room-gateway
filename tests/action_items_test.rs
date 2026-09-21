use thought_khoral_room_gateway::action_items::extract_action_items;

#[test]
fn extracts_only_explicit_action_lines_after_the_agent_mention() {
    let result = extract_action_items(
        "@action-items\n- Prepare rollout checklist | owner: Maya | due: Friday\nDiscussion follows.",
    )
    .expect("well-formed explicit action line must parse");

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].text, "Prepare rollout checklist");
    assert_eq!(result[0].owner.as_deref(), Some("Maya"));
    assert_eq!(result[0].due.as_deref(), Some("Friday"));
}

#[test]
fn rejects_an_empty_optional_action_field() {
    assert!(extract_action_items("@action-items\n- Prepare rollout | owner: ").is_err());
}
