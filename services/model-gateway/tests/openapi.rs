//! docs/openapi/model-gateway.v1.yaml cannot drift from the routes the gateway registers: the operations in the file
//! are exactly the routes in its table, every `$ref` resolves, and the Problem schema lists exactly the error
//! codes the code has. The same checks as the other files.

use askbar_proto::ErrorCode;
use serde_json::Value;
use std::collections::BTreeSet;

fn load() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi/model-gateway.v1.yaml");
    serde_saphyr::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("model-gateway.v1.yaml is not YAML: {e}"))
}

fn operations(spec: &Value) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        for (key, op) in item.as_object().unwrap() {
            if ["get", "post", "put", "delete", "patch"].contains(&key.as_str()) {
                assert!(op["responses"].as_object().is_some_and(|r| !r.is_empty()), "{key} {path} has no responses");
                out.insert((key.to_uppercase(), path.clone()));
            }
        }
    }
    out
}

fn refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "$ref" {
                    out.push(x.as_str().unwrap().to_string());
                } else {
                    refs(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

#[test]
fn the_model_gateway_api_file_describes_exactly_the_gateways_routes() {
    let spec = load();
    assert!(spec["openapi"].as_str().is_some_and(|v| v.starts_with("3.1.")), "model-gateway.v1.yaml must declare OpenAPI 3.1");
    let routes: BTreeSet<(String, String)> = askbar_model_gateway::routes().into_iter().map(|r| (r.method.to_string(), r.path)).collect();
    let ops = operations(&spec);
    let missing_from_spec: Vec<_> = routes.difference(&ops).collect();
    let missing_from_gateway: Vec<_> = ops.difference(&routes).collect();
    assert!(missing_from_spec.is_empty(), "routes the gateway has and the file does not describe: {missing_from_spec:?}");
    assert!(missing_from_gateway.is_empty(), "operations in the file the gateway does not have: {missing_from_gateway:?}");

    let mut ids = BTreeSet::new();
    for item in spec["paths"].as_object().unwrap().values() {
        for (k, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str() {
                assert!(ids.insert(id.to_string()), "operationId {id} is used twice ({k})");
            }
        }
    }
    let mut found = Vec::new();
    refs(&spec, &mut found);
    for r in found {
        let mut cur = &spec;
        for part in r.trim_start_matches("#/").split('/') {
            cur = &cur[part];
        }
        assert!(!cur.is_null(), "{r} does not resolve");
    }
    let listed: BTreeSet<String> = spec["components"]["schemas"]["Problem"]["properties"]["error"]["enum"]
        .as_array()
        .expect("Problem.error has an enum")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    let real: BTreeSet<String> = ErrorCode::ALL.iter().map(|c| c.as_str().to_string()).collect();
    assert_eq!(listed, real, "the Problem codes are not the codes in askbar-proto");
}

#[test]
fn every_path_is_under_the_gateway_prefix_and_only_health_needs_no_token() {
    let spec = load();
    assert!(spec["paths"].as_object().unwrap().keys().all(|p| p.starts_with("/api/model-gateway/v1/")), "gateway paths");
    let mut open = Vec::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (key, op) in item.as_object().unwrap() {
            if op["security"].as_array().is_some_and(|s| s.is_empty()) {
                open.push(format!("{key} {path}"));
            }
        }
    }
    assert_eq!(open, ["get /api/model-gateway/v1/health"], "only the health check is open");
    // The chat route can answer the refusal that stops model work.
    assert!(spec["paths"]["/api/model-gateway/v1/chat"]["post"]["responses"]["503"].is_object());
}

#[test]
fn the_files_event_names_are_the_ones_the_code_sends() {
    use askbar_proto::gateway::ChatEvent;
    let spec = load();
    let listed: BTreeSet<String> = spec["components"]["schemas"]["ChatEvent"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["properties"]["event"]["const"].as_str().unwrap().to_string())
        .collect();
    let real: BTreeSet<String> = [
        ChatEvent::Span { span_id: String::new(), trace_id: String::new() },
        ChatEvent::Delta { text: String::new() },
        ChatEvent::Usage { input: None, output: None, cache_read: None, cache_write: None, estimated: false },
        ChatEvent::Done { finish_reason: None, ttft_ms: None, attempts: 0 },
        ChatEvent::Error { code: String::new(), class: askbar_proto::gateway::FailureClass::Fatal, retry_after_secs: None, attempts: 0 },
    ]
    .iter()
    .map(|e| e.name().to_string())
    .collect();
    assert_eq!(listed, real);
}
