//! `antibody doctor`: configuration and connectivity checks (no secrets printed).

use crate::config::{Config, env};
use crate::{client, git};
use std::time::Duration;

fn status(ok: bool) -> &'static str {
    if ok { "ok" } else { "FAIL" }
}

pub fn run() -> i32 {
    let mut failed = false;
    for (name, required) in [
        ("ANTIBODY_URL", true),
        ("ANTIBODY_TOKEN", true),
        ("ANTIBODY_REPOSITORY", false),
        ("ANTIBODY_COMPONENT", false),
        ("ANTIBODY_ORG", false),
        ("ANTIBODY_CI_SECRET", false),
    ] {
        let set = env(name).is_some();
        println!(
            "{:<22} {}",
            name,
            if set {
                "set"
            } else if required {
                "MISSING (required)"
            } else {
                "not set"
            }
        );
        failed |= required && !set;
    }
    let cwd = std::env::current_dir().ok();
    let info = cwd.as_deref().and_then(git::collect);
    match git::resolve_repository(info.as_ref()) {
        Some(r) => println!("repository in use      {r}"),
        None => println!(
            "repository in use      none (not a git repo with an origin; set ANTIBODY_REPOSITORY)"
        ),
    }
    let Ok(cfg) = Config::from_env() else {
        return 1;
    };
    let Ok(http) = client::http(Duration::from_secs(10)) else {
        return 1;
    };
    let ready = http.get(client::url(&cfg, "/v1/health/ready")).send();
    let ok = ready
        .as_ref()
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    println!("server ready           {}", status(ok));
    failed |= !ok;
    let repos = client::authed(http.get(client::url(&cfg, "/v1/repositories")), &cfg).send();
    match repos {
        Ok(r) if r.status().is_success() => println!("token valid            ok"),
        Ok(r) => {
            println!("token valid            FAIL (http {})", r.status().as_u16());
            failed = true;
        }
        Err(_) => {
            println!("token valid            FAIL (request failed)");
            failed = true;
        }
    }
    i32::from(failed)
}
