use super::*;

#[test]
fn session_end_hands_off_before_slow_git_and_still_uploads() {
    use std::os::unix::fs::PermissionsExt;
    let (url, seen) = serve(vec![(202, ACCEPTED.into())]);
    let tmp = Tmp::new("slowgit");
    let fake_bin = tmp.0.join("bin");
    std::fs::create_dir(&fake_bin).unwrap();
    let fake_git = fake_bin.join("git");
    std::fs::write(
        &fake_git,
        "#!/bin/sh\ncase \"$3 $4\" in\n\
         'rev-parse --is-inside-work-tree') sleep 4; echo true;;\n\
         'remote get-url') echo https://example.test/acme/api.git;;\n\
         'rev-parse HEAD') echo 0123456789012345678901234567890123456789;;\n\
         'rev-parse --abbrev-ref') echo main;;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(fake_git, std::fs::Permissions::from_mode(0o700)).unwrap();
    let transcript = tmp.0.join("transcript.jsonl");
    std::fs::write(
        &transcript,
        include_bytes!("../fixtures/claude_transcript.jsonl"),
    )
    .unwrap();
    let hook = serde_json::json!({
        "session_id": "slow-git-session", "cwd": tmp.0, "transcript_path": transcript
    });
    let mut cmd = bin();
    cmd.args(["hook", "session-end", "--agent", "claude-code"])
        .env("ANTIBODY_URL", &url)
        .env("ANTIBODY_TOKEN", "tok")
        .env("ANTIBODY_HOME", tmp.0.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()));
    let started = Instant::now();
    assert_eq!(run_with_stdin(cmd, &hook.to_string()), (0, String::new()));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(seen.lock().unwrap().is_empty());
    let deadline = Instant::now() + Duration::from_secs(12);
    while seen.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "slow Git must not discard the session");
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["external_id"], "slow-git-session");
    assert_eq!(body["repository"], "acme/api");
}
