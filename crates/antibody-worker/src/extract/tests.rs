use super::llm::parse_json_object;
use super::*;
use antibody_core::api::SessionHints;
use antibody_core::api::TranscriptMessage;
use antibody_core::applicability::DependencyVersion;
use antibody_core::model::AgentType;
use antibody_core::model::MessageRole;

fn msg(role: MessageRole, content: &str) -> TranscriptMessage {
    TranscriptMessage {
        role,
        content: content.into(),
        tool_name: None,
    }
}

fn req(messages: Vec<TranscriptMessage>, hints: Option<SessionHints>) -> SessionIngestRequest {
    SessionIngestRequest {
        agent_type: AgentType::ClaudeCode,
        external_id: None,
        repository: "acme/api".into(),
        revision: Some("abc".into()),
        branch: Some("fix".into()),
        worktree_dirty: false,
        component_path: Some("services/api".into()),
        messages,
        hints,
        started_at: None,
        ended_at: None,
    }
}

#[test]
fn heuristic_extracts_from_error_and_final_message() {
    let r = req(
        vec![
            msg(MessageRole::User, "cargo test hangs and then fails"),
            msg(
                MessageRole::Tool,
                "thread 'db::tests' panicked at src/db.rs:10:5:\nError: PoolTimedOut while waiting for sqlx::PgPool",
            ),
            msg(
                MessageRole::Assistant,
                "The root cause was that the tokio test runtime used a single worker thread. I switched to multi_thread flavor in src/db.rs and all tests pass now.",
            ),
        ],
        Some(SessionHints {
            dependencies: vec![
                DependencyVersion {
                    name: "sqlx".into(),
                    version: "0.8.2".into(),
                    ecosystem: None,
                },
                DependencyVersion {
                    name: "left-pad".into(),
                    version: "1.0.0".into(),
                    ecosystem: None,
                },
            ],
            ..Default::default()
        }),
    );
    let ex = heuristic(&r);
    assert_eq!(ex.candidates.len(), 1);
    let c = &ex.candidates[0];
    assert!(!c.error_signatures.is_empty());
    assert!(c.remedy.contains("multi_thread"));
    assert!(
        c.root_cause
            .as_deref()
            .unwrap()
            .contains("single worker thread")
    );
    let versioned = heuristic(&req(
        vec![
            msg(MessageRole::Tool, "error[E0308]: mismatched types"),
            msg(
                MessageRole::Assistant,
                "The root cause was that sqlx 0.8 now infers nullable columns as Option. Fixed the cast.",
            ),
        ],
        None,
    ));
    assert!(
        versioned.candidates[0]
            .root_cause
            .as_deref()
            .unwrap()
            .contains("nullable columns"),
        "periods inside versions do not end the sentence"
    );
    assert!(c.paths.contains(&"src/db.rs".to_string()));
    assert!(c.claims_tests_passed);
    assert_eq!(
        c.applicability.dependencies.len(),
        1,
        "only dependencies mentioned in the session"
    );
    assert_eq!(c.applicability.dependencies[0].version_req, "^0.8.2");
    assert_eq!(c.applicability.components, vec!["services/api/**"]);
}

#[test]
fn heuristic_does_not_invent_knowledge() {
    let r = req(
        vec![
            msg(MessageRole::User, "rename this variable"),
            msg(MessageRole::Assistant, "Done, renamed it everywhere."),
        ],
        None,
    );
    assert!(heuristic(&r).candidates.is_empty());
}

#[test]
fn heuristic_handles_multibyte_error_signature_prefixes() {
    for signature in [
        "error: abcdefghijklmnopqrstuvéconnection failed",
        "error: abcdefghijklmnopqrstuv接続に失敗しました",
        "error: abcdefghijklmnopqrstuv🚨connection failed",
    ] {
        assert!(!signature.is_char_boundary(30));
        let symptom = "The application cannot connect";
        let r = req(
            vec![
                msg(MessageRole::User, symptom),
                msg(MessageRole::Tool, signature),
                msg(
                    MessageRole::Assistant,
                    "Corrected the connection settings and restarted the application.",
                ),
            ],
            None,
        );
        let ex = heuristic(&r);

        assert_eq!(ex.candidates.len(), 1);
        assert_eq!(ex.candidates[0].symptom, format!("{symptom} — {signature}"));
        assert_eq!(ex.candidates[0].error_signatures, vec![signature]);
    }
}

#[test]
fn heuristic_does_not_repeat_unicode_signature_already_in_symptom() {
    let signature = "error: abcdefghijklmnopqrstuv接続に失敗しました";
    let r = req(
        vec![
            msg(MessageRole::User, signature),
            msg(
                MessageRole::Assistant,
                "Corrected the connection settings and restarted the application.",
            ),
        ],
        None,
    );
    let ex = heuristic(&r);

    assert_eq!(ex.candidates.len(), 1);
    assert_eq!(ex.candidates[0].symptom, signature);
}

#[test]
fn parses_fenced_llm_json() {
    let raw = "```json\n{\"problems\":[{\"title\":\"t\",\"symptom\":\"s\",\"remedy\":\"do the thing properly\"}]}\n```";
    let out = parse_json_object(raw).unwrap();
    assert_eq!(out.problems.len(), 1);
    assert!(parse_json_object("no json here").is_none());
}

#[test]
fn sanitize_redacts_model_output() {
    let c = sanitize(Candidate {
        title: "x".into(),
        symptom: "s".into(),
        remedy: "set GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        ..Default::default()
    });
    assert!(!c.remedy.contains("ghp_aaaa"));
}
