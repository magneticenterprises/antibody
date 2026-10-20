use antibody_core::applicability::{
    Applicability, CallerContext, DependencyConstraint, DependencyVersion, evaluate,
};
use antibody_core::model::ApplicabilityStatus;
use antibody_core::redact::{redact, redact_json};
use serde_json::json;

#[test]
fn quoted_secret_assignments_preserve_json_and_remove_whole_values() {
    let input = r#"{"password":"synthetic credential with spaces","api_key":"synthetic\"escaped-value","ordinary":"keep me"}"#;
    let redacted = redact(input);
    let parsed: serde_json::Value = serde_json::from_str(&redacted.text).unwrap();
    assert_eq!(parsed["password"], "[REDACTED:secret_assignment]");
    assert_eq!(parsed["api_key"], "[REDACTED:secret_assignment]");
    assert_eq!(parsed["ordinary"], "keep me");
    assert!(!redacted.text.contains("synthetic"));
    assert!(
        !redact("'client_secret': 'synthetic value'")
            .text
            .contains("synthetic")
    );
}

#[test]
fn structured_credentials_are_redacted_by_key_even_without_token_patterns() {
    let mut value = json!({
        "password": "tiny",
        "nested": [{"api_key": "synthetic arbitrary value"}, {"access_token": 12345}],
        "private_key": {"parts": ["synthetic", "credential"]},
        "messages": [{"content": "ordinary context"}],
        "context_tokens": 123,
        "empty_secret": null,
    });
    assert_eq!(redact_json(&mut value), 4);
    assert_eq!(value["password"], "[REDACTED:secret_assignment]");
    assert_eq!(
        value["nested"][1]["access_token"],
        "[REDACTED:secret_assignment]"
    );
    assert_eq!(value["private_key"], "[REDACTED:secret_assignment]");
    assert_eq!(value["messages"][0]["content"], "ordinary context");
    assert_eq!(value["context_tokens"], 123);
    assert_eq!(redact_json(&mut value), 0);
}

#[test]
fn component_matching_respects_path_boundaries_and_reports_definite_mismatches() {
    for component in ["services/api/**", "services/api"] {
        let app = Applicability {
            components: vec![component.into()],
            ..Default::default()
        };
        for path in ["services/api", "services/api/src/main.rs"] {
            assert_eq!(
                evaluate(
                    &app,
                    false,
                    None,
                    &CallerContext {
                        component_path: Some(path),
                        same_repository: true,
                        ..Default::default()
                    }
                )
                .status,
                ApplicabilityStatus::Compatible,
                "{component}: {path}",
            );
        }
        let ctx = CallerContext {
            component_path: Some("services/api2/src/main.rs"),
            same_repository: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate(&app, false, None, &ctx).status,
            ApplicabilityStatus::Incompatible
        );
        assert_eq!(
            evaluate(
                &app,
                false,
                None,
                &CallerContext {
                    same_repository: false,
                    ..ctx
                }
            )
            .status,
            ApplicabilityStatus::Unknown,
        );
        assert_eq!(
            evaluate(&app, false, None, &CallerContext::default()).status,
            ApplicabilityStatus::Unknown,
        );
    }
}

#[test]
fn dependency_ecosystem_does_not_match_a_same_named_package_elsewhere() {
    let app = Applicability {
        dependencies: vec![DependencyConstraint {
            name: "shared-package".into(),
            ecosystem: Some("cargo".into()),
            version_req: "^1".into(),
        }],
        ..Default::default()
    };
    let wrong = [DependencyVersion {
        name: "shared-package".into(),
        ecosystem: Some("npm".into()),
        version: "1.2.0".into(),
    }];
    let ctx = CallerContext {
        dependencies: &wrong,
        ..Default::default()
    };
    assert_eq!(
        evaluate(&app, false, None, &ctx).status,
        ApplicabilityStatus::Unknown
    );
    let correct = [
        wrong[0].clone(),
        DependencyVersion {
            ecosystem: Some("CARGO".into()),
            version: "2.0.0".into(),
            ..wrong[0].clone()
        },
    ];
    let ctx = CallerContext {
        dependencies: &correct,
        ..Default::default()
    };
    assert_eq!(
        evaluate(&app, false, None, &ctx).status,
        ApplicabilityStatus::Incompatible
    );
}

#[test]
fn applicability_fingerprints_preserve_scope_and_condition_differences() {
    let base = Applicability {
        dependencies: vec![DependencyConstraint {
            name: "shared-package".into(),
            ecosystem: Some("cargo".into()),
            version_req: "^1".into(),
        }],
        components: vec!["services/api/**".into()],
        conditions: Some("feature flag enabled".into()),
        ..Default::default()
    };
    let mut changed = base.clone();
    changed.dependencies[0].ecosystem = Some("npm".into());
    assert_ne!(base.key(), changed.key());
    changed = base.clone();
    changed.components = vec!["services/api2/**".into()];
    assert_ne!(base.key(), changed.key());
    changed = base.clone();
    changed.conditions = Some("feature flag disabled".into());
    assert_ne!(base.key(), changed.key());
    let ordered = Applicability {
        components: vec!["b/**".into(), "./a/**".into()],
        platforms: vec!["Linux".into(), "macos".into()],
        ..Default::default()
    };
    let reordered = Applicability {
        components: vec!["a/**".into(), "b/**".into(), "b/**".into()],
        platforms: vec!["macos".into(), "linux".into()],
        ..Default::default()
    };
    assert_eq!(ordered.key(), reordered.key());
}
