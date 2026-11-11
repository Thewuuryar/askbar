//! docs/openapi/alerter.v1.yaml cannot drift from the routes the gateway registers: the operations in the file
//! are exactly the routes in its table, every `$ref` resolves, and the Problem schema lists exactly the error
//! codes the code has. The same checks as the other files.

use askbar_proto::ErrorCode;
use serde_json::Value;
use std::collections::BTreeSet;

fn load() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi/alerter.v1.yaml");
    serde_saphyr::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("alerter.v1.yaml is not YAML: {e}"))
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
fn the_alerter_api_file_describes_exactly_the_alerters_routes() {
    let spec = load();
    assert!(spec["openapi"].as_str().is_some_and(|v| v.starts_with("3.1.")), "alerter.v1.yaml must declare OpenAPI 3.1");
    let routes: BTreeSet<(String, String)> =
        askbar_alerter::routes().into_iter().map(|r| (r.method.to_string(), r.path.replace('*', "{id}"))).collect();
    let ops = operations(&spec);
    let missing_from_spec: Vec<_> = routes.difference(&ops).collect();
    let missing_from_alerter: Vec<_> = ops.difference(&routes).collect();
    assert!(missing_from_spec.is_empty(), "routes the alerter has and the file does not describe: {missing_from_spec:?}");
    assert!(missing_from_alerter.is_empty(), "operations in the file the alerter does not have: {missing_from_alerter:?}");

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
fn every_path_is_under_the_alerter_prefix_and_only_health_needs_no_token() {
    let spec = load();
    assert!(spec["paths"].as_object().unwrap().keys().all(|p| p.starts_with("/api/alerter/v1/")), "alerter paths");
    let mut open = Vec::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (key, op) in item.as_object().unwrap() {
            if op["security"].as_array().is_some_and(|s| s.is_empty()) {
                open.push(format!("{key} {path}"));
            }
        }
    }
    assert_eq!(open, ["get /api/alerter/v1/health"], "only the health check is open");
}

/// A kind in the code and a kind in the file are the same list, so adding one in only one place fails here.
#[test]
fn the_files_kinds_severities_and_channels_are_the_ones_the_code_has() {
    use askbar_proto::alerter::Kind;
    let spec = load();
    let strings = |v: &Value| -> BTreeSet<String> { v.as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string()).collect() };
    let real: BTreeSet<String> = Kind::ALL.iter().map(|k| k.name().to_string()).collect();
    assert_eq!(strings(&spec["components"]["schemas"]["Kind"]["enum"]), real);
    assert_eq!(strings(&spec["components"]["schemas"]["Severity"]["enum"]), ["info", "warning", "critical"].map(String::from).into());
    assert_eq!(strings(&spec["components"]["schemas"]["Outcome"]["enum"]), ["sent", "failed", "suppressed"].map(String::from).into());
    // The file's field names are the Fields struct's.
    let listed: BTreeSet<String> = spec["components"]["schemas"]["Fields"]["properties"].as_object().unwrap().keys().cloned().collect();
    let full = serde_json::to_value(askbar_proto::alerter::Fields {
        link: Some("a".into()),
        service: Some("a".into()),
        period: Some(askbar_proto::alerter::Period::Day),
        percent: Some(80),
        count: Some(1),
        minutes: Some(1),
        free_mb: Some(1),
        version: Some("1".into()),
    })
    .unwrap();
    assert_eq!(listed, full.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>());
}
