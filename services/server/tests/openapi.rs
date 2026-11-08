//! docs/openapi/client.v1.yaml and server.v1.yaml cannot drift from the routes the server registers:
//! the operations in each file are exactly the routes in its table, and the Problem schema lists exactly
//! the error codes the code has.
use askbar_proto::ErrorCode;
use askbar_server::routes::{client_routes, server_routes, RouteEntry};
use serde_json::Value;
use std::collections::BTreeSet;

fn load(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi").join(name);
    serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{name} is not YAML: {e}"))
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

fn table(rows: Vec<RouteEntry>) -> BTreeSet<(String, String)> {
    // The one ANY route (the old /api/v1 prefix) is deliberately not described.
    rows.into_iter().filter(|r| r.method != "ANY").map(|r| (r.method, r.path)).collect()
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

fn check_spec(name: &str, spec: &Value, routes: BTreeSet<(String, String)>) {
    assert!(spec["openapi"].as_str().is_some_and(|v| v.starts_with("3.1.")), "{name} must declare OpenAPI 3.1");
    let ops = operations(spec);
    let missing_from_spec: Vec<_> = routes.difference(&ops).collect();
    let missing_from_server: Vec<_> = ops.difference(&routes).collect();
    assert!(missing_from_spec.is_empty(), "{name}: routes the server has and the file does not describe: {missing_from_spec:?}");
    assert!(missing_from_server.is_empty(), "{name}: operations in the file the server does not have: {missing_from_server:?}");
    // Operation ids are unique.
    let mut ids = BTreeSet::new();
    for item in spec["paths"].as_object().unwrap().values() {
        for (k, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str() {
                assert!(ids.insert(id.to_string()), "{name}: operationId {id} is used twice ({k})");
            }
        }
    }
    // Every $ref points at something that exists.
    let mut found = Vec::new();
    refs(spec, &mut found);
    for r in found {
        let mut cur = spec;
        for part in r.trim_start_matches("#/").split('/') {
            cur = &cur[part];
        }
        assert!(!cur.is_null(), "{name}: {r} does not resolve");
    }
    // The Problem schema lists exactly the codes the code has.
    let listed: BTreeSet<String> = spec["components"]["schemas"]["Problem"]["properties"]["error"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: Problem.error has no enum"))
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    let real: BTreeSet<String> = ErrorCode::ALL.iter().map(|c| c.as_str().to_string()).collect();
    assert_eq!(listed, real, "{name}: the Problem codes are not the codes in askbar-proto");
}

#[test]
fn the_client_api_file_describes_exactly_the_clients_routes() {
    check_spec("client.v1.yaml", &load("client.v1.yaml"), table(client_routes()));
}

#[test]
fn the_server_api_file_describes_exactly_the_servers_routes() {
    check_spec("server.v1.yaml", &load("server.v1.yaml"), table(server_routes()));
}

#[test]
fn every_path_in_the_files_is_under_its_apis_prefix_and_the_stream_is_described() {
    let client = load("client.v1.yaml");
    assert!(client["paths"].as_object().unwrap().keys().all(|p| p.starts_with("/api/client/v1/")), "client paths");
    let chat = &client["paths"]["/api/client/v1/spaces/{space_id}/chat"]["post"]["responses"]["200"];
    assert!(chat["$ref"].as_str().is_some_and(|r| r.ends_with("EventStream")));
    assert!(client["components"]["responses"]["EventStream"]["content"]["text/event-stream"].is_object());
    let server = load("server.v1.yaml");
    assert!(server["paths"].as_object().unwrap().keys().all(|p| p.starts_with("/api/server/v1/")), "server paths");
}
