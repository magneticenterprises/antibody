//! The event header is unsigned. Match it to discriminating signed body fields
//! before allowing the body to reserve its permanent replay fingerprint.
use serde_json::Value;

pub(super) fn matches_event(event: &str, body: &Value) -> bool {
    let object = |key: &str| body.get(key).is_some_and(Value::is_object);
    let action = body.get("action").and_then(Value::as_str).unwrap_or("");
    let (allowed, required): (&[&str], bool) = match event {
        "push" => (
            &["ref", "after", "before", "commits", "head_commit", "pusher"],
            body.get("ref").is_some_and(Value::is_string)
                && body.get("after").is_some_and(Value::is_string),
        ),
        "pull_request" => (&["pull_request", "number"], object("pull_request")),
        "check_run" => (&["check_run"], object("check_run")),
        "workflow_run" => (&["workflow_run"], object("workflow_run")),
        "deployment_status" => (
            &["deployment", "deployment_status"],
            object("deployment") && object("deployment_status"),
        ),
        "repository" => (
            &[],
            object("repository")
                && matches!(
                    action,
                    "archived"
                        | "unarchived"
                        | "deleted"
                        | "renamed"
                        | "transferred"
                        | "privatized"
                        | "publicized"
                        | "created"
                        | "edited"
                ),
        ),
        "member" => (
            &["member"],
            object("member") && matches!(action, "added" | "edited" | "removed"),
        ),
        "team_add" => (&["team"], object("team")),
        "team" => (
            &["team"],
            object("team")
                && matches!(
                    action,
                    "added_to_repository"
                        | "removed_from_repository"
                        | "created"
                        | "deleted"
                        | "edited"
                ),
        ),
        "membership" => (
            &["member", "team"],
            object("member") && object("team") && matches!(action, "added" | "removed"),
        ),
        "organization" => (
            &["membership", "invitation"],
            (object("membership") && matches!(action, "member_added" | "member_removed"))
                || (object("invitation") && action == "member_invited"),
        ),
        _ => return true,
    };
    const DISCRIMINATORS: &[&str] = &[
        "ref",
        "after",
        "before",
        "commits",
        "head_commit",
        "pusher",
        "pull_request",
        "number",
        "check_run",
        "workflow_run",
        "deployment",
        "deployment_status",
        "member",
        "team",
        "membership",
        "invitation",
    ];
    required
        && DISCRIMINATORS
            .iter()
            .all(|key| body.get(*key).is_none() || allowed.contains(key))
}
