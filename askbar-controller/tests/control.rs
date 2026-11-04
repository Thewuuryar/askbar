//! The controller against a fake Docker on a unix socket: what it may touch, who may ask, and what it refuses.

use askbar_controller::{docker::Docker, router, Config};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "ctl_test-token";
const PROJECT: &str = "askbar";

struct Fake {
    /// (id, project, service, state)
    containers: Mutex<Vec<(String, String, String, String)>>,
    calls: Mutex<Vec<String>>,
}

fn id(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

async fn json_list(State(f): State<Arc<Fake>>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let filters: Value = serde_json::from_str(&q["filters"]).unwrap();
    let want = filters["label"][0].as_str().unwrap().to_string();
    let rows: Vec<Value> = f
        .containers
        .lock()
        .unwrap()
        .iter()
        .filter(|c| format!("com.docker.compose.project={}", c.1) == want)
        .map(|c| json!({ "Id": c.0, "State": c.3, "Status": if c.3 == "running" { "Up 1 hour (healthy)" } else { "Exited (0)" }, "Labels": { "com.docker.compose.project": c.1, "com.docker.compose.service": c.2 } }))
        .collect();
    Json(json!(rows))
}

async fn inspect(Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    if id == "self" {
        (StatusCode::OK, Json(json!({ "Config": { "Labels": { "com.docker.compose.project": PROJECT } } })))
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "message": "No such container" })))
    }
}

async fn change(State(f): State<Arc<Fake>>, Path((cid, verb)): Path<(String, String)>) -> StatusCode {
    f.calls.lock().unwrap().push(format!("{verb} {cid}"));
    let mut cs = f.containers.lock().unwrap();
    let Some(c) = cs.iter_mut().find(|c| c.0 == cid) else { return StatusCode::NOT_FOUND };
    match verb.as_str() {
        "start" if c.3 == "running" => return StatusCode::NOT_MODIFIED,
        "stop" if c.3 != "running" => return StatusCode::NOT_MODIFIED,
        "start" | "restart" => c.3 = "running".into(),
        "stop" => c.3 = "exited".into(),
        _ => return StatusCode::NOT_FOUND,
    }
    StatusCode::NO_CONTENT
}

struct T {
    url: String,
    http: reqwest::Client,
    fake: Arc<Fake>,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-ctl-{}", std::process::id() as u64 * 1000 + rand_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(Fake {
        containers: Mutex::new(vec![
            (id(1), PROJECT.into(), "server".into(), "running".into()),
            (id(2), PROJECT.into(), "caddy".into(), "running".into()),
            (id(3), PROJECT.into(), "cpa".into(), "exited".into()),
            (id(4), PROJECT.into(), "admin".into(), "running".into()),
            (id(5), PROJECT.into(), "controller".into(), "running".into()),
            (id(6), "other".into(), "server".into(), "running".into()),
        ]),
        calls: Mutex::new(vec![]),
    });
    let app = Router::new()
        .route("/containers/json", get(json_list))
        .route("/containers/{id}/json", get(inspect))
        .route("/containers/{id}/{verb}", post(change))
        .with_state(fake.clone());
    let sock = dir.join("docker.sock");
    let l = tokio::net::UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let hash_file = dir.join("token.sha256");
    std::fs::write(&hash_file, Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
    let cfg = Arc::new(Config { docker: Docker::new(&sock), project: PROJECT.into(), token_hash_file: hash_file });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/control/v1", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, router(cfg)).await.unwrap() });
    T { url, http: reqwest::Client::new(), fake, dir }
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() as u64
}

impl T {
    async fn get(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut r = self.http.get(format!("{}{path}", self.url));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        let r = r.send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
    async fn post(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut r = self.http.post(format!("{}{path}", self.url));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        let r = r.send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn it_lists_only_the_three_services_of_its_own_project() {
    let t = start().await;
    let (s, v) = t.get("/services", Some(TOKEN)).await;
    assert_eq!(s, 200);
    let names: Vec<_> = v["services"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["server", "caddy", "cpa"], "the admin and the controller are not listed");
    assert_eq!(v["services"][0]["state"], "running");
    assert_eq!(v["services"][0]["health"], "healthy");
    assert_eq!(v["services"][2]["state"], "exited");
}

#[tokio::test]
async fn start_stop_and_restart_act_on_the_right_container_and_report_the_result() {
    let t = start().await;
    let (s, v) = t.post("/services/cpa/start", Some(TOKEN)).await;
    assert_eq!((s, v["state"].as_str()), (200, Some("running")));
    let (s, v) = t.post("/services/server/stop", Some(TOKEN)).await;
    assert_eq!((s, v["state"].as_str()), (200, Some("exited")));
    let (s, _) = t.post("/services/server/restart", Some(TOKEN)).await;
    assert_eq!(s, 200);
    assert_eq!(*t.fake.calls.lock().unwrap(), vec![format!("start {}", id(3)), format!("stop {}", id(1)), format!("restart {}", id(1))]);
    // Already in the wanted state is fine.
    assert_eq!(t.post("/services/caddy/start", Some(TOKEN)).await.0, 200);
}

#[tokio::test]
async fn nothing_outside_the_allow_list_or_the_project_can_be_named() {
    let t = start().await;
    for name in ["admin", "controller", "mock-llm", "..%2Fadmin", "other"] {
        assert_eq!(t.post(&format!("/services/{name}/stop"), Some(TOKEN)).await.0, 404, "{name}");
    }
    for action in ["kill", "exec", "remove", "pause"] {
        assert_eq!(t.post(&format!("/services/server/{action}"), Some(TOKEN)).await.0, 404, "{action}");
    }
    assert!(t.fake.calls.lock().unwrap().is_empty(), "Docker was never asked");
    // The other project's `server` is untouched even though it has the same service name.
    t.post("/services/server/restart", Some(TOKEN)).await;
    assert_eq!(*t.fake.calls.lock().unwrap(), vec![format!("restart {}", id(1))]);
}

#[tokio::test]
async fn a_token_is_needed_and_a_wrong_one_or_the_hash_is_refused() {
    let t = start().await;
    assert_eq!(t.get("/services", None).await.0, 401);
    assert_eq!(t.get("/services", Some("wrong")).await.0, 401);
    let hash: String = Sha256::digest(TOKEN.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(t.get("/services", Some(&hash)).await.0, 401, "the stored hash is not a credential");
    assert_eq!(t.post("/services/cpa/start", None).await.0, 401);
    assert_eq!(t.get("/health", None).await.0, 200, "health needs no token");
    // Fails closed when the hash file goes away.
    std::fs::remove_file(t.dir.join("token.sha256")).unwrap();
    assert_eq!(t.get("/services", Some(TOKEN)).await.0, 401);
}

#[tokio::test]
async fn docker_being_away_is_a_502_problem() {
    let t = start().await;
    let cfg = Arc::new(Config {
        docker: Docker::new(t.dir.join("nothing.sock")),
        project: PROJECT.into(),
        token_hash_file: t.dir.join("token.sha256"),
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/control/v1/services", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, router(cfg)).await.unwrap() });
    let r = t.http.get(url).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(r.status(), 502);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "server_unavailable");
}

#[tokio::test]
async fn it_finds_its_own_project_and_will_not_listen_on_every_network() {
    let t = start().await;
    let d = Docker::new(t.dir.join("docker.sock"));
    assert_eq!(d.project_of("self").await.unwrap(), PROJECT);
    assert!(d.project_of("../etc").await.is_err());
    assert!(askbar_controller::parse_addr("0.0.0.0:8083").is_err());
    assert!(askbar_controller::parse_addr("[::]:8083").is_err());
    assert!(askbar_controller::parse_addr("172.32.0.2:8083").is_ok());
}
