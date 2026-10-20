use super::*;

#[test]
fn workflow_rerun_preserves_attempt_and_provider_ordering() {
    let body = serde_json::json!({
        "action": "completed",
        "workflow_run": {
            "id": 42, "run_attempt": 3, "name": "tests", "conclusion": "failure",
            "head_sha": "abc123", "updated_at": "2026-10-20T03:04:05Z"
        },
        "repository": { "full_name": "acme/api" }
    });
    let event = parse_event("workflow_run", body.to_string().as_bytes()).unwrap();
    let GitEvent::CheckCompleted {
        run_attempt,
        completed_at,
        ..
    } = event
    else {
        panic!("expected completed workflow");
    };
    assert_eq!(run_attempt, 3);
    assert_eq!(
        completed_at.unwrap().to_rfc3339(),
        "2026-10-20T03:04:05+00:00"
    );
}

#[test]
fn parses_failed_check_run() {
    let body = serde_json::json!({
        "action": "completed",
        "check_run": {
            "id": 42, "name": "test", "conclusion": "failure", "head_sha": "abc123",
            "html_url": "https://github.com/acme/api/runs/42",
            "output": {"title": "1 failing", "summary": "error[E0308]: mismatched types", "text": null},
            "check_suite": {"head_branch": "fix/pool"}
        },
        "repository": {"full_name": "acme/api", "id": 7, "default_branch": "main"}
    });
    let ev = parse_event("check_run", body.to_string().as_bytes()).unwrap();
    match ev {
        GitEvent::CheckCompleted {
            name,
            conclusion,
            head_sha,
            excerpt,
            repo,
            id,
            head_branch,
            ..
        } => {
            assert_eq!(name, "test");
            assert!(is_failure(&conclusion));
            assert_eq!(head_sha, "abc123");
            assert_eq!(id, "42");
            assert_eq!(head_branch.as_deref(), Some("fix/pool"));
            assert!(excerpt.unwrap().contains("E0308"));
            assert_eq!(repo.provider_id.as_deref(), Some("7"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parses_push_with_revert() {
    let body = serde_json::json!({
        "ref": "refs/heads/main", "after": "def456",
        "repository": {"full_name": "acme/api", "default_branch": "main"},
        "commits": [{"id": "def456", "message": "Revert \"x\"\n\nThis reverts commit abc1234.", "added": [], "modified": ["Cargo.lock"], "removed": []}]
    });
    let ev = parse_event("push", body.to_string().as_bytes()).unwrap();
    let GitEvent::Push {
        branch,
        commits,
        default_branch_push,
        ..
    } = ev
    else {
        panic!()
    };
    assert_eq!(branch.as_deref(), Some("main"));
    assert!(default_branch_push);
    assert_eq!(commits[0].changed_paths, vec!["Cargo.lock"]);
}

#[test]
fn unknown_events_are_unsupported_not_errors() {
    assert!(matches!(
        parse_event("star", b"{}").unwrap(),
        GitEvent::Unsupported { .. }
    ));
    assert!(parse_event("push", b"not json").is_err());
}
