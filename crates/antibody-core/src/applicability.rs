//! Applicability conditions attached to solutions, and their evaluation
//! against a caller's context (dependencies, component, platform).

use crate::model::ApplicabilityStatus;
use globset::Glob;
use serde::{Deserialize, Serialize};

/// A dependency version constraint under which a fix is known to apply.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema,
)]
pub struct DependencyConstraint {
    /// Package name, e.g. `tokio` or `react`.
    pub name: String,
    /// Ecosystem, e.g. `cargo`, `npm`, `pypi`. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    /// Semver requirement, e.g. `>=1.30, <2` or `^18`.
    pub version_req: String,
}

/// A dependency version present in the caller's environment.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema,
)]
pub struct DependencyVersion {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
}

/// Conditions under which a solution applies.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
    schemars::JsonSchema,
)]
pub struct Applicability {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DependencyConstraint>,
    /// Path globs of components the fix touched, e.g. `services/api/**`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Platforms the fix is known to apply to, e.g. `linux`, `macos`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    /// Free-text conditions (untrusted, display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<String>,
}

impl Applicability {
    /// Canonical key used in fingerprints: two solutions to the same problem
    /// with different applicability are kept as distinct versions.
    pub fn key(&self) -> String {
        let mut deps: Vec<_> = self
            .dependencies
            .iter()
            .map(|d| {
                (
                    d.ecosystem.as_ref().map(|e| e.trim().to_lowercase()),
                    d.name.to_lowercase(),
                    normalize_req(&d.version_req),
                )
            })
            .collect();
        deps.sort();
        deps.dedup();
        let mut components: Vec<_> = self
            .components
            .iter()
            .map(|c| {
                crate::normalize::normalize_path(c)
                    .trim_end_matches('/')
                    .to_owned()
            })
            .collect();
        components.sort();
        components.dedup();
        let mut platforms: Vec<String> = self.platforms.iter().map(|p| p.to_lowercase()).collect();
        platforms.sort();
        platforms.dedup();
        serde_json::json!({
            "dependencies": deps,
            "components": components,
            "platforms": platforms,
            "conditions": self.conditions.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        })
        .to_string()
    }

    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty()
            && self.components.is_empty()
            && self.platforms.is_empty()
            && self
                .conditions
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
    }
}

/// Context supplied by the caller at retrieval time.
#[derive(Debug, Clone, Default)]
pub struct CallerContext<'a> {
    pub dependencies: &'a [DependencyVersion],
    pub component_path: Option<&'a str>,
    pub files: &'a [String],
    pub platform: Option<&'a str>,
    pub same_repository: bool,
}

/// Result of evaluating applicability for one candidate.
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema,
)]
pub struct ApplicabilityAssessment {
    pub status: ApplicabilityStatus,
    /// Human-readable conditions the caller should check before reusing.
    pub conditions: Vec<String>,
    /// Why the status was chosen.
    pub notes: Vec<String>,
    /// Multiplier applied to the ranking score (0..=1.2).
    #[serde(skip)]
    #[schemars(skip)]
    pub score_factor: f32,
}

fn normalize_req(req: &str) -> String {
    let trimmed = req.trim();
    // Accept npm-style space separated comparators: ">=1.2 <2" -> ">=1.2, <2".
    let mut out = String::new();
    let mut prev_was_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            prev_was_space = true;
            continue;
        }
        if prev_was_space && matches!(ch, '<' | '>' | '=' | '^' | '~') && !out.ends_with(',') {
            out.push_str(", ");
        } else if prev_was_space {
            out.push(' ');
        }
        prev_was_space = false;
        out.push(ch);
    }
    out
}

fn parse_version(v: &str) -> Option<semver::Version> {
    let v = v.trim().trim_start_matches(['v', '=', '^', '~']);
    if let Ok(parsed) = semver::Version::parse(v) {
        return Some(parsed);
    }
    let parts: Vec<&str> = v.split('.').collect();
    let padded = match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => return None,
    };
    semver::Version::parse(&padded).ok()
}

fn parse_req(r: &str) -> Option<semver::VersionReq> {
    semver::VersionReq::parse(&normalize_req(r)).ok()
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    let path = crate::normalize::normalize_path(path);
    let pattern = crate::normalize::normalize_path(pattern);
    match Glob::new(&pattern) {
        Ok(g) => {
            g.compile_matcher().is_match(&path)
                // `dir/**` also includes the directory itself. A literal
                // component accepts descendants only at a path boundary.
                || pattern.strip_suffix("/**").is_some_and(|root| root == path)
                || (!pattern.is_empty()
                    && !pattern.contains(['*', '?', '[', ']', '{', '}'])
                    && path
                        .strip_prefix(pattern.trim_end_matches('/'))
                        .is_some_and(|suffix| suffix.starts_with('/')))
        }
        Err(_) => false,
    }
}

/// Evaluate a solution's applicability against the caller context.
pub fn evaluate(
    app: &Applicability,
    needs_revalidation: bool,
    revalidation_reason: Option<&str>,
    ctx: &CallerContext<'_>,
) -> ApplicabilityAssessment {
    let mut status = ApplicabilityStatus::Compatible;
    let mut conditions = Vec::new();
    let mut notes = Vec::new();
    let mut factor: f32 = 1.0;

    let escalate =
        |current: ApplicabilityStatus, next: ApplicabilityStatus| -> ApplicabilityStatus {
            fn sev(s: ApplicabilityStatus) -> u8 {
                match s {
                    ApplicabilityStatus::Compatible => 0,
                    ApplicabilityStatus::Unknown => 1,
                    ApplicabilityStatus::Stale => 2,
                    ApplicabilityStatus::Incompatible => 3,
                }
            }
            if sev(next) > sev(current) {
                next
            } else {
                current
            }
        };

    for dep in &app.dependencies {
        conditions.push(format!("{} {}", dep.name, dep.version_req));
        let caller = ctx
            .dependencies
            .iter()
            .filter(|d| {
                d.name.eq_ignore_ascii_case(&dep.name)
                    && match (&dep.ecosystem, &d.ecosystem) {
                        (Some(expected), Some(actual)) => expected.eq_ignore_ascii_case(actual),
                        _ => true,
                    }
            })
            // Prefer an explicitly matching ecosystem to an unspecified
            // legacy entry when the caller supplies both.
            .max_by_key(|d| {
                matches!((&dep.ecosystem, &d.ecosystem), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
            });
        match (caller, parse_req(&dep.version_req)) {
            (Some(have), Some(req)) => match parse_version(&have.version) {
                Some(v) if req.matches(&v) => {
                    notes.push(format!(
                        "{} {} satisfies {}",
                        dep.name, have.version, dep.version_req
                    ));
                    factor *= 1.1;
                }
                Some(_) => {
                    notes.push(format!(
                        "fix applies to {} {}, but you have {}",
                        dep.name, dep.version_req, have.version
                    ));
                    status = escalate(status, ApplicabilityStatus::Incompatible);
                    factor *= 0.2;
                }
                None => {
                    notes.push(format!(
                        "could not compare {} version {}",
                        dep.name, have.version
                    ));
                    status = escalate(status, ApplicabilityStatus::Unknown);
                }
            },
            (None, _) => {
                notes.push(format!(
                    "check that {} matches {}",
                    dep.name, dep.version_req
                ));
                status = escalate(status, ApplicabilityStatus::Unknown);
                factor *= 0.95;
            }
            (Some(_), None) => {
                notes.push(format!("unparseable constraint for {}", dep.name));
                status = escalate(status, ApplicabilityStatus::Unknown);
            }
        }
    }

    if !app.platforms.is_empty() {
        conditions.push(format!("platform: {}", app.platforms.join(", ")));
        match ctx.platform {
            Some(p) if app.platforms.iter().any(|x| x.eq_ignore_ascii_case(p)) => {}
            Some(p) => {
                notes.push(format!(
                    "fix observed on {}, you are on {p}",
                    app.platforms.join("/")
                ));
                status = escalate(status, ApplicabilityStatus::Incompatible);
                factor *= 0.3;
            }
            None => status = escalate(status, ApplicabilityStatus::Unknown),
        }
    }

    if !app.components.is_empty() {
        conditions.push(format!("component: {}", app.components.join(", ")));
        let mut probes: Vec<&str> = ctx.files.iter().map(String::as_str).collect();
        if let Some(c) = ctx.component_path {
            probes.push(c);
        }
        if !probes.is_empty() {
            let hit = app
                .components
                .iter()
                .any(|g| probes.iter().any(|p| glob_matches(g, p)));
            if hit {
                notes.push("touches the same component".into());
                factor *= 1.15;
            } else if ctx.same_repository {
                notes.push(format!(
                    "fix was in {}, a different component",
                    app.components.join(", ")
                ));
                status = escalate(status, ApplicabilityStatus::Incompatible);
                factor *= 0.2;
            } else {
                notes.push(
                    "component differs; check the corresponding code in this repository".into(),
                );
                status = escalate(status, ApplicabilityStatus::Unknown);
                factor *= 0.85;
            }
        } else {
            status = escalate(status, ApplicabilityStatus::Unknown);
        }
    }

    if !ctx.same_repository {
        notes.push("from another repository: adapt rather than copy".into());
        factor *= 0.9;
    }

    if needs_revalidation {
        status = escalate(status, ApplicabilityStatus::Stale);
        notes.push(format!(
            "needs revalidation: {}",
            revalidation_reason.unwrap_or("evidence may be stale")
        ));
        factor *= 0.6;
    }

    if let Some(c) = &app.conditions {
        conditions.push(crate::text::one_line(c, 200));
    }

    ApplicabilityAssessment {
        status,
        conditions,
        notes,
        score_factor: factor.clamp(0.0, 1.2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokio_fix() -> Applicability {
        Applicability {
            dependencies: vec![DependencyConstraint {
                name: "tokio".into(),
                ecosystem: Some("cargo".into()),
                version_req: ">=1.30 <2".into(),
            }],
            components: vec!["services/api/**".into()],
            ..Default::default()
        }
    }

    #[test]
    fn compatible_when_constraints_hold() {
        let deps = vec![DependencyVersion {
            name: "tokio".into(),
            version: "1.38.0".into(),
            ecosystem: None,
        }];
        let files = vec!["services/api/src/main.rs".to_string()];
        let a = evaluate(
            &tokio_fix(),
            false,
            None,
            &CallerContext {
                dependencies: &deps,
                files: &files,
                same_repository: true,
                ..Default::default()
            },
        );
        assert_eq!(a.status, ApplicabilityStatus::Compatible, "{a:?}");
        assert!(a.score_factor > 1.0);
    }

    #[test]
    fn incompatible_when_version_outside_range() {
        let deps = vec![DependencyVersion {
            name: "tokio".into(),
            version: "0.2.25".into(),
            ecosystem: None,
        }];
        let a = evaluate(
            &tokio_fix(),
            false,
            None,
            &CallerContext {
                dependencies: &deps,
                same_repository: true,
                ..Default::default()
            },
        );
        assert_eq!(a.status, ApplicabilityStatus::Incompatible);
        assert!(a.notes.iter().any(|n| n.contains("you have 0.2.25")));
        assert!(a.score_factor < 0.5);
    }

    #[test]
    fn unknown_when_caller_omits_dependency() {
        let a = evaluate(
            &tokio_fix(),
            false,
            None,
            &CallerContext {
                same_repository: true,
                ..Default::default()
            },
        );
        assert_eq!(a.status, ApplicabilityStatus::Unknown);
    }

    #[test]
    fn stale_when_revalidation_needed() {
        let deps = vec![DependencyVersion {
            name: "tokio".into(),
            version: "1.38".into(),
            ecosystem: None,
        }];
        let a = evaluate(
            &tokio_fix(),
            true,
            Some("fix commit was reverted"),
            &CallerContext {
                dependencies: &deps,
                same_repository: true,
                ..Default::default()
            },
        );
        assert_eq!(a.status, ApplicabilityStatus::Stale);
        assert!(a.notes.iter().any(|n| n.contains("reverted")));
    }

    #[test]
    fn applicability_key_ignores_order_and_spacing() {
        let a = Applicability {
            dependencies: vec![
                DependencyConstraint {
                    name: "B".into(),
                    ecosystem: None,
                    version_req: ">=1 <2".into(),
                },
                DependencyConstraint {
                    name: "a".into(),
                    ecosystem: None,
                    version_req: "^3".into(),
                },
            ],
            ..Default::default()
        };
        let b = Applicability {
            dependencies: vec![
                DependencyConstraint {
                    name: "a".into(),
                    ecosystem: None,
                    version_req: "^3".into(),
                },
                DependencyConstraint {
                    name: "b".into(),
                    ecosystem: None,
                    version_req: ">=1, <2".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(a.key(), b.key());
    }
}
