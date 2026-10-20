//! Contract drift: the committed OpenAPI document and MCP tool schemas must
//! match what the Rust types generate (A18). Regenerate with:
//!   cargo run -p antibody-api -- openapi > openapi/openapi.json
//!   cargo run -p antibody-api -- mcp-schema > openapi/mcp-tools.json

fn committed(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../openapi")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()))
}

#[test]
fn openapi_document_has_not_drifted() {
    assert_eq!(
        committed("openapi.json").trim_end(),
        antibody_api::openapi::json().trim_end(),
        "openapi/openapi.json is stale; regenerate it"
    );
}

#[test]
fn mcp_tool_schemas_have_not_drifted() {
    assert_eq!(
        committed("mcp-tools.json").trim_end(),
        antibody_api::openapi::mcp_tools_json().trim_end(),
        "openapi/mcp-tools.json is stale; regenerate it"
    );
}

#[test]
fn mcp_exposes_exactly_the_three_safe_tools() {
    let names: Vec<String> = antibody_api::mcp::tool_definitions()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(
        names,
        ["find_relevant_context", "get_solution", "report_outcome"]
    );
}

#[test]
fn spec_endpoints_are_documented() {
    let doc: serde_json::Value = serde_json::from_str(&antibody_api::openapi::json()).unwrap();
    for (path, method) in [
        ("/v1/sessions", "post"),
        ("/v1/sessions/{id}", "get"),
        ("/v1/webhooks/github", "post"),
        ("/v1/context/search", "post"),
        ("/v1/solutions/{id}", "get"),
        ("/v1/outcomes", "post"),
        ("/v1/repositories", "get"),
        ("/v1/health/live", "get"),
        ("/v1/health/ready", "get"),
    ] {
        assert!(
            doc["paths"][path][method].is_object(),
            "{method} {path} missing from OpenAPI"
        );
    }
}

fn doc() -> serde_json::Value {
    serde_json::from_str(&antibody_api::openapi::json()).unwrap()
}

#[test]
fn operation_ids_are_unique() {
    let d = doc();
    let mut seen = std::collections::BTreeMap::new();
    for (path, item) in d["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str()
                && let Some(prev) = seen.insert(id.to_string(), format!("{method} {path}"))
            {
                panic!("operationId {id} used by both {prev} and {method} {path}");
            }
        }
    }
}

macro_rules! assert_enum_schema {
    ($d:expr, $($t:ident),+) => {$(
        let schema: Vec<String> = $d["components"]["schemas"][stringify!($t)]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{} has no enum schema", stringify!($t)))
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let serde: Vec<String> = antibody_core::model::$t::ALL
            .iter()
            .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(schema, serde, "OpenAPI enum for {} differs from its wire format", stringify!($t));
    )+};
}

#[test]
fn enum_schemas_match_the_wire_format() {
    let d = doc();
    assert_enum_schema!(
        d,
        Scope,
        AgentType,
        SolutionState,
        MergeState,
        DeploymentState,
        Role,
        PrincipalKind,
        Permission,
        RepositoryProvider,
        CaptureMode,
        ExtractionStatus,
        MessageRole,
        EvidenceKind,
        EvidenceResult,
        OutcomeKind,
        OutcomeVerification,
        WorkflowState,
        ApplicabilityStatus,
        SourceKind
    );
}

#[test]
fn authenticated_operations_document_auth_errors() {
    let d = doc();
    for (path, item) in d["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            let secured = op["security"].as_array().is_some_and(|s| !s.is_empty());
            if secured {
                for code in ["401", "403", "429"] {
                    assert!(
                        op["responses"][code].is_object(),
                        "{method} {path} does not document {code}"
                    );
                }
            }
        }
    }
}
