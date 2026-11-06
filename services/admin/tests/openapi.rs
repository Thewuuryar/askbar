#![cfg(unix)]
//! docs/openapi/admin.v1.yaml cannot drift from the routes the admin registers: the operations in the
//! file are exactly the API routes, the Problem schema lists exactly the error codes, and what the file
//! says about who may call each operation is what the admin does.
use askbar_admin::{api_key, routes, AdminDb, AdminState};
use askbar_proto::ErrorCode;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn spec() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi/admin.v1.yaml");
    serde_yaml_ng::from_str(&std::fs::read_to_string(&path).unwrap()).expect("admin.v1.yaml is YAML")
}

/// (METHOD, path) -> the security alternatives written for it ([] means open; None means the file's default).
fn operations(spec: &Value) -> BTreeMap<(String, String), Option<Vec<String>>> {
    let mut out = BTreeMap::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (key, op) in item.as_object().unwrap() {
            if ["get", "post", "put", "delete", "patch"].contains(&key.as_str()) {
                assert!(op["responses"].as_object().is_some_and(|r| !r.is_empty()), "{key} {path} has no responses");
                let sec = op.get("security").map(|s| s.as_array().unwrap().iter().flat_map(|alt| alt.as_object().unwrap().keys().cloned()).collect::<Vec<_>>());
                out.insert((key.to_uppercase(), path.clone()), sec);
            }
        }
    }
    out
}

fn refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => m.iter().for_each(|(k, x)| if k == "$ref" { out.push(x.as_str().unwrap().into()) } else { refs(x, out) }),
        Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
        _ => {}
    }
}

#[test]
fn the_file_describes_exactly_the_admin_api_routes_and_its_problem_codes_are_the_real_ones() {
    let spec = spec();
    assert!(spec["openapi"].as_str().is_some_and(|v| v.starts_with("3.1.")));
    let in_file: BTreeSet<(String, String)> = operations(&spec).into_keys().collect();
    let registered: BTreeSet<(String, String)> = routes::api_route_table().into_iter().map(|r| (r.method, r.path)).collect();
    let a: Vec<_> = registered.difference(&in_file).collect();
    let b: Vec<_> = in_file.difference(&registered).collect();
    assert!(a.is_empty(), "routes the admin has and the file does not describe: {a:?}");
    assert!(b.is_empty(), "operations in the file the admin does not have: {b:?}");
    let mut found = Vec::new();
    refs(&spec, &mut found);
    for r in found {
        let mut cur = &spec;
        for part in r.trim_start_matches("#/").split('/') {
            cur = &cur[part];
        }
        assert!(!cur.is_null(), "{r} does not resolve");
    }
    let listed: BTreeSet<String> = spec["components"]["schemas"]["Problem"]["properties"]["error"]["enum"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    let real: BTreeSet<String> = ErrorCode::ALL.iter().map(|c| c.as_str().to_string()).collect();
    assert_eq!(listed, real);
    let mut ids = BTreeSet::new();
    for item in spec["paths"].as_object().unwrap().values() {
        for op in item.as_object().unwrap().values() {
            if let Some(id) = op["operationId"].as_str() {
                assert!(ids.insert(id.to_string()), "operationId {id} twice");
            }
        }
    }
}

#[tokio::test]
async fn what_the_file_says_about_a_key_is_what_the_admin_does() {
    let spec = spec();
    let state = AdminState::new(AdminDb::open_in_memory().unwrap());
    let key = {
        let db = state.db.lock().await;
        api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap().1
    };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_admin::router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let mut session_only = BTreeSet::new();
    let mut open = BTreeSet::new();
    for ((method, path), security) in operations(&spec) {
        let concrete = path.replace("{id}", "x").replace("{span_id}", "x");
        let call = |key: Option<&str>| {
            let mut r = http.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{url}{concrete}")).header("x-askbar-admin", "1");
            if let Some(k) = key {
                r = r.bearer_auth(k);
            }
            r.send()
        };
        match security.as_deref() {
            Some([]) => {
                open.insert((method.clone(), path.clone()));
                let s = call(None).await.unwrap().status().as_u16();
                assert!(s != 401, "{method} {path} is open in the file but answered 401");
            }
            Some(schemes) if schemes == ["adminSession"] => {
                session_only.insert((method.clone(), path.clone()));
                assert_eq!(call(Some(&key)).await.unwrap().status(), 401, "{method} {path} is session-only in the file but a key got in");
            }
            _ => {
                // Default (session or key): a key gets past authentication (anything but 401).
                let s = call(Some(&key)).await.unwrap().status().as_u16();
                assert!(s != 401, "{method} {path} says a key may call it, but it answered 401");
            }
        }
    }
    let want_session_only: BTreeSet<(String, String)> = [("GET", "/session"), ("DELETE", "/session"), ("PUT", "/password"), ("GET", "/keys"), ("POST", "/keys"), ("DELETE", "/keys/{id}"), ("GET", "/services"), ("POST", "/services/{name}/{action}"), ("GET", "/cpa"), ("POST", "/cpa/logins"), ("GET", "/cpa/logins/{state}"), ("POST", "/cpa/logins/{state}/callback"), ("DELETE", "/cpa/accounts/{name}")]
        .into_iter()
        .map(|(m, p)| (m.to_string(), format!("/admin/api/v1{p}")))
        .collect();
    assert_eq!(session_only, want_session_only, "the routes a key may not call");
    assert_eq!(open.len(), 4, "health, setup (read), setup password and sign-in are open: {open:?}");
}
