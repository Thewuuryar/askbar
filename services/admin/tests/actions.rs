#![cfg(unix)]
//! The record of what the admin does (ADR 0010, plan step 2.7): every change is appended to the auditor before it is
//! made, and a change whose record cannot be appended is refused and changes nothing.

mod common;
use askbar_admin::actions::{classify, ActionLog, NotRecorded};
use askbar_admin::routes::api_route_table;
use axum::http::Method;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// Routes that change nothing the admin keeps: signing in and out.
const NOT_CHANGES: [(&str, &str); 2] = [("POST", "/api/admin/v1/session"), ("DELETE", "/api/admin/v1/session")];

#[test]
fn every_route_that_changes_something_has_a_category_and_no_other_route_does() {
    let table = api_route_table();
    let mut classified = BTreeSet::new();
    for r in &table {
        let method: Method = r.method.parse().unwrap();
        // Reads that are recorded: the backup, and the content of a sealed span (each item viewed, ADR 0011 rule 12).
        let changes =
            method != Method::GET || ["/api/admin/v1/backup", "/api/admin/v1/audit/payloads/{span_id}"].contains(&r.path.as_str());
        let exempt = NOT_CHANGES.contains(&(r.method.as_str(), r.path.as_str()));
        match (changes && !exempt, classify(&method, &r.path)) {
            (true, Some(_)) => {
                classified.insert((r.method.clone(), r.path.clone()));
            }
            (true, None) => panic!("{} {} changes something and has no category in actions::classify", r.method, r.path),
            (false, Some(c)) => panic!("{} {} is not a change but is classified as {c:?}", r.method, r.path),
            (false, None) => {}
        }
    }
    // And nothing in `classify` names a route that is not there (a renamed route would silently lose its record).
    let have: BTreeSet<(String, String)> = table.iter().map(|r| (r.method.clone(), r.path.clone())).collect();
    for (m, p) in &classified {
        assert!(have.contains(&(m.clone(), p.clone())));
    }
    assert!(classified.len() >= 20, "{classified:?}");
}

fn actions(t: &common::T) -> Vec<Value> {
    t.rig
        .rows("SELECT id, service, category, action, outcome, detail_json FROM admin_actions ORDER BY at, rowid")
        .into_iter()
        .map(|r| json!({ "id": r[0], "service": r[1], "category": r[2], "action": r[3], "outcome": r[4], "detail": serde_json::from_str::<Value>(r[5].as_str().unwrap_or("null")).unwrap() }))
        .collect()
}

#[tokio::test]
async fn a_change_is_recorded_before_it_is_made_and_again_with_how_it_ended() {
    let t = common::start(true).await;
    let (st, made) = t.call(M::POST, "/api/admin/v1/users", json!({ "name": "Alice" })).await;
    assert_eq!(st, 201);
    let alice = made["id"].as_str().unwrap().to_string();
    // A refused change (a name that is taken) is recorded as refused, with the status.
    assert_eq!(t.call(M::POST, "/api/admin/v1/users", json!({ "name": "Alice" })).await.0, 409);
    // A change with a parameter in its path records the id.
    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pairing-keys"), json!({ "name": "laptop" })).await.0, 200);
    // A read is not a change.
    assert_eq!(t.call(M::GET, "/api/admin/v1/users", json!(null)).await.0, 200);

    let rows = actions(&t);
    let of = |category: &str, outcome: &str| rows.iter().filter(|r| r["category"] == category && r["outcome"] == outcome).count();
    assert_eq!((of("users", "requested"), of("users", "done"), of("users", "refused")), (2, 1, 1), "{rows:#?}");
    assert_eq!((of("pairing_keys", "requested"), of("pairing_keys", "done")), (1, 1), "{rows:#?}");
    assert!(rows.iter().filter(|r| r["category"] != "config").all(|r| r["service"] == "admin"), "the caller is the token's");
    // The server, which the change was pushed to, recorded the version it applied with its own token (ADR 0028 rule 5).
    let mut config = rows.clone();
    for _ in 0..100 {
        if config.iter().any(|r| r["category"] == "config" && r["service"] == "server" && r["detail"]["version"].is_i64()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        config = actions(&t);
    }
    assert!(config.iter().any(|r| r["category"] == "config" && r["service"] == "server" && r["detail"]["version"].is_i64()), "{config:#?}");
    assert!(!rows.iter().any(|r| r["detail"]["route"]
        .as_str()
        .is_some_and(|p| p.ends_with("/users") && r["outcome"] == "requested" && r["detail"]["method"] == "GET")));

    let asked = rows.iter().find(|r| r["category"] == "pairing_keys" && r["outcome"] == "requested").unwrap();
    assert_eq!(asked["detail"]["route"], "/api/admin/v1/users/{id}/pairing-keys");
    assert_eq!(asked["detail"]["params"]["id"], json!(alice));
    assert_eq!(asked["detail"]["by"], "session");
    let ended = rows.iter().find(|r| r["category"] == "pairing_keys" && r["outcome"] == "done").unwrap();
    assert_eq!((ended["detail"]["request"].clone(), ended["detail"]["status"].clone()), (asked["id"].clone(), json!(200)));
    let refused = rows.iter().find(|r| r["outcome"] == "refused").unwrap();
    assert_eq!(refused["detail"]["status"], 409);
}

#[tokio::test]
async fn no_secret_and_no_body_is_in_a_record() {
    let t = common::start(true).await;
    t.call(
        M::PUT,
        "/api/admin/v1/setup/audit-key",
        json!({ "passphrase": "SECRET-PASSPHRASE-1234567890", "confirm": "SECRET-PASSPHRASE-1234567890" }),
    )
    .await;
    t.call(
        M::POST,
        "/api/admin/v1/providers",
        json!({ "name": "mock", "type": "openai", "base_url": "http://x/v1", "model": "m", "api_key": "sk-PROVIDER-KEY-123" }),
    )
    .await;
    t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "SCRIPT-NAME" })).await;
    t.call(
        M::PUT,
        "/api/admin/v1/settings",
        json!({ "system_prompt": "SYSTEM-PROMPT-TEXT", "display": askbar_proto::DisplaySettings::default() }),
    )
    .await;
    let all = serde_json::to_string(&actions(&t)).unwrap();
    assert!(all.contains("audit_key_setup") && all.contains("api_key_create") && all.contains("\"settings\""), "{all}");
    for secret in ["SECRET-PASSPHRASE", "sk-PROVIDER-KEY", "SCRIPT-NAME", "SYSTEM-PROMPT-TEXT"] {
        assert!(!all.contains(secret), "{secret} is in the records");
    }
}

#[tokio::test]
async fn a_key_is_recorded_as_a_key_and_a_request_that_is_not_let_in_is_not_recorded_at_all() {
    let t = common::start(true).await;
    let (_, made) = t.call(M::POST, "/api/admin/v1/keys", json!({ "name": "ci" })).await;
    let key = made["key"].as_str().unwrap().to_string();
    let before = actions(&t).len();
    // Signed out: refused before anything is recorded.
    let r = t
        .http
        .post(format!("{}/api/admin/v1/users", t.adm))
        .header("x-askbar-admin", "1")
        .json(&json!({ "name": "Eve" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert_eq!(actions(&t).len(), before, "a request that is not let in leaves no record");
    // A key may change things (and is asked who it is).
    let r = t.http.post(format!("{}/api/admin/v1/users", t.adm)).bearer_auth(&key).json(&json!({ "name": "Bob" })).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let rows = actions(&t);
    let asked = rows.iter().rev().find(|r| r["category"] == "users" && r["outcome"] == "requested").unwrap();
    assert_eq!(asked["detail"]["by"], "api_key");
}

#[tokio::test]
async fn the_backup_download_is_recorded_and_other_reads_are_not() {
    let t = common::start(true).await;
    let get = |p: &'static str| {
        let (http, url, cookie) = (t.http.clone(), format!("{}{p}", t.adm), t.cookie.clone());
        async move { http.get(url).header("cookie", cookie).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(get("/api/admin/v1/providers").await, 200);
    assert_eq!(get("/api/admin/v1/settings").await, 200);
    assert!(actions(&t).is_empty());
    assert_eq!(get("/api/admin/v1/backup").await, 200);
    let rows = actions(&t);
    assert_eq!(
        rows.iter().map(|r| (r["category"].as_str().unwrap(), r["outcome"].as_str().unwrap())).collect::<Vec<_>>(),
        [("backup", "requested"), ("backup", "done")]
    );
}

#[tokio::test]
async fn when_the_auditor_is_away_a_change_is_refused_with_a_sentence_and_changes_nothing() {
    let t = common::start(true).await;
    // Point the admin's record link at nothing; everything else is as it was.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let token = askbar_admin::actions::append_token(&t.rig.admin_tokens).unwrap();
    let admin = askbar_admin::AdminState::build(
        askbar_admin::AdminDb::open_in_memory().unwrap(),
        None,
        askbar_admin::secrets::AdminSecrets::ephemeral(),
    );
    askbar_admin::web::init_password(&*admin.db.lock().await, common::PW).unwrap();
    admin.actions.set(ActionLog::new(&dead, token)).ok().unwrap();
    let adm = common::serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();

    for (m, path, body) in [
        (M::POST, "/api/admin/v1/users", Some(json!({ "name": "Alice" }))),
        (
            M::PUT,
            "/api/admin/v1/settings",
            Some(json!({ "system_prompt": "changed", "display": askbar_proto::DisplaySettings::default() })),
        ),
        (M::POST, "/api/admin/v1/keys", Some(json!({ "name": "ci" }))),
        (M::POST, "/api/admin/v1/providers", Some(json!({ "name": "p", "type": "openai", "base_url": "http://x/v1", "model": "m" }))),
    ] {
        let mut r = http.request(m.clone(), format!("{adm}{path}")).header("x-askbar-admin", "1").header("cookie", &cookie);
        if let Some(b) = &body {
            r = r.json(b);
        }
        let resp = r.send().await.unwrap();
        assert_eq!(resp.status(), 503, "{m} {path}");
        let problem: Value = resp.json().await.unwrap();
        assert!(problem["detail"].as_str().unwrap().contains("nothing was changed"), "{problem}");
    }
    let db = admin.db.lock().await;
    let count = |sql: &str| db.conn().query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM users"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM providers"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM admin_api_keys"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM settings WHERE key = 'system_prompt'"), 0, "the setting was not saved");
    drop(db);
    // Reading still works: the admin does not go dark with the auditor.
    let r = http.get(format!("{adm}/api/admin/v1/providers")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    drop(t);
}

/// A stub auditor that answers `/admin-actions` with one status and remembers what it was sent.
async fn stub(status: u16) -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/api/audit/v1/admin-actions",
            post(move |State(s): State<Arc<Mutex<Vec<Value>>>>, Json(b): Json<Value>| async move {
                s.lock().unwrap().push(b);
                StatusCode::from_u16(status).unwrap()
            }),
        )
        .with_state(seen.clone());
    (common::serve(app).await, seen)
}

#[tokio::test]
async fn the_auditors_answer_decides_which_sentence_the_person_gets() {
    async fn record(log: &ActionLog) -> Result<(), NotRecorded> {
        let r = askbar_proto::audit::AdminActionStart {
            id: "x".into(),
            at: 1,
            category: "c".into(),
            action: "a".into(),
            outcome: "requested".into(),
            detail: None,
        };
        log.record(&r).await
    }
    let (ok, seen) = stub(201).await;
    assert_eq!(record(&ActionLog::new(&ok, "t".into())).await, Ok(()));
    assert_eq!(seen.lock().unwrap().len(), 1);
    let (busy, _) = stub(503).await;
    assert_eq!(record(&ActionLog::new(&busy, "t".into())).await, Err(NotRecorded::Backlog));
    let (no, _) = stub(403).await;
    assert_eq!(record(&ActionLog::new(&no, "t".into())).await, Err(NotRecorded::Rejected));
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    assert_eq!(record(&ActionLog::new(&dead, "t".into())).await, Err(NotRecorded::Unreachable));
    for why in [NotRecorded::Backlog, NotRecorded::Rejected, NotRecorded::Unreachable] {
        assert!(why.sentence().contains("nothing was changed"));
    }
}

#[tokio::test]
async fn an_action_the_admin_takes_itself_is_recorded_first_and_is_not_done_without_it() {
    let (url, seen) = stub(201).await;
    let log = ActionLog::new(&url, "t".into());
    let done = log
        .around(
            "config_push",
            "gateway",
            json!({ "target": "gateway" }),
            async { Ok::<_, ()>(5) },
            |r| json!({ "version": r.as_ref().ok() }),
        )
        .await;
    assert_eq!(done, Ok(Ok(5)));
    let outcomes: Vec<String> = seen.lock().unwrap().iter().map(|b| b["outcome"].as_str().unwrap().to_string()).collect();
    assert_eq!(outcomes, ["requested", "done"]);
    assert_eq!(seen.lock().unwrap()[1]["detail"]["version"], 5, "the end record carries what the result shows");
    let failed = log.around("config_push", "gateway", json!({}), async { Err::<(), _>("no") }, |_| json!({})).await;
    assert_eq!(failed, Ok(Err("no")));
    assert_eq!(seen.lock().unwrap().last().unwrap()["outcome"], "failed");

    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let ran = Arc::new(Mutex::new(false));
    let flag = ran.clone();
    let blocked = ActionLog::new(&dead, "t".into())
        .around(
            "config_push",
            "gateway",
            json!({}),
            async move {
                *flag.lock().unwrap() = true;
                Ok::<_, ()>(())
            },
            |_| json!({}),
        )
        .await;
    assert_eq!(blocked, Err(NotRecorded::Unreachable));
    assert!(!*ran.lock().unwrap(), "the action did not run");
}

#[tokio::test]
async fn a_push_to_the_server_and_one_to_the_gateway_are_actions_of_their_own() {
    let t = common::start(true).await;
    assert_eq!(t.call(M::POST, "/api/admin/v1/users", json!({ "name": "Alice" })).await.0, 201);
    let rows = actions(&t);
    let push =
        |outcome: &str| rows.iter().find(|r| r["category"] == "config_push" && r["action"] == "server" && r["outcome"] == outcome).cloned();
    let (asked, done) = (push("requested").expect("requested"), push("done").expect("done"));
    assert_eq!(done["detail"]["request"], asked["id"]);
    assert!(done["detail"]["version"].as_u64().is_some(), "the end names the version the server was given: {done}");
    // The push came after the change was requested and before it was done.
    let order: Vec<(&str, &str)> = rows
        .iter()
        .filter(|r| r["category"] != "config")
        .map(|r| (r["category"].as_str().unwrap(), r["outcome"].as_str().unwrap()))
        .collect();
    assert_eq!(order, [("users", "requested"), ("config_push", "requested"), ("config_push", "done"), ("users", "done")], "{rows:#?}");

    // The gateway is given its configuration by the admin's own loop, which is also an action.
    t.call(
        M::PUT,
        "/api/admin/v1/setup/audit-key",
        json!({ "passphrase": "a passphrase of at least twenty", "confirm": "a passphrase of at least twenty" }),
    )
    .await;
    let sync = t.admin.gateway.get().unwrap().sync(&t.admin).await;
    assert!(matches!(sync, askbar_admin::gateway_link::Sync::Pushed(_)), "{sync:?}");
    let rows = actions(&t);
    let gw: Vec<&str> = rows
        .iter()
        .filter(|r| r["category"] == "config_push" && r["action"] == "model-gateway")
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(gw, ["requested", "done"], "{rows:#?}");
}

#[tokio::test]
async fn a_push_to_the_gateway_waits_when_it_cannot_be_recorded_and_is_made_when_it_can() {
    use std::sync::atomic::{AtomicU16, Ordering};
    let t = common::start(true).await;
    // An auditor whose answer the test chooses, in front of an admin of its own (so the real one is not disturbed).
    let status = Arc::new(AtomicU16::new(201));
    let s = status.clone();
    let stub = common::serve(Router::new().route(
        "/api/audit/v1/admin-actions",
        post(move || {
            let s = s.clone();
            async move { StatusCode::from_u16(s.load(Ordering::Relaxed)).unwrap() }
        }),
    ))
    .await;
    let server_token = askbar_admin::server_link::read_token(&t.dir.join("admin-token")).unwrap();
    let admin = askbar_admin::AdminState::build(
        askbar_admin::AdminDb::open_in_memory().unwrap(),
        Some(askbar_admin::server_link::ServerLink::new(&t.server_api_url, server_token)),
        askbar_admin::secrets::AdminSecrets::ephemeral(),
    );
    askbar_admin::web::init_password(&*admin.db.lock().await, common::PW).unwrap();
    admin.actions.set(ActionLog::new(&stub, "t".into())).ok().unwrap();
    // The admin has an audit key (it is what the gateway cannot run without), and a link to the gateway of the first.
    let adm = common::serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let key = json!({ "passphrase": "a passphrase of at least twenty", "confirm": "a passphrase of at least twenty" });
    let r = http
        .put(format!("{adm}/api/admin/v1/setup/audit-key"))
        .header("x-askbar-admin", "1")
        .header("cookie", cookie)
        .json(&key)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let gateway = t.admin.gateway.get().unwrap().clone();

    // The auditor takes nothing: the push is not made and the gateway runs nothing.
    status.store(503, Ordering::Relaxed);
    let sync = gateway.sync(&admin).await;
    assert!(matches!(sync, askbar_admin::gateway_link::Sync::NotReady(_)), "{sync:?}");
    assert!(t.gateway.config_version().is_none(), "the gateway was given nothing");
    // The auditor takes it again: the next round pushes.
    status.store(201, Ordering::Relaxed);
    let sync = gateway.sync(&admin).await;
    assert!(matches!(sync, askbar_admin::gateway_link::Sync::Pushed(_)), "{sync:?}");
    assert!(t.gateway.config_version().is_some());
}

#[tokio::test]
async fn a_settings_change_records_the_retention_and_the_kill_switch_but_not_the_prompt() {
    let t = common::start(true).await;
    let body = json!({ "system_prompt": "NOT-IN-THE-RECORD", "display": askbar_proto::DisplaySettings::default(), "audit_retention_days": 45, "kill_switch": true });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", body).await.0, 204);
    let rows = actions(&t);
    let done = rows.iter().find(|r| r["category"] == "settings" && r["outcome"] == "done").unwrap();
    assert_eq!((done["detail"]["audit_retention_days"].clone(), done["detail"]["kill_switch"].clone()), (json!(45), json!(true)));
    assert!(!serde_json::to_string(&rows).unwrap().contains("NOT-IN-THE-RECORD"));
}

/// `askbar-admin reset` run as a program, with its own data directory and a token directory `bootstrap` made.
fn run_reset(dir: &std::path::Path, auditor_url: &str) -> (bool, String) {
    let bin = env!("CARGO_BIN_EXE_askbar-admin");
    let out = std::process::Command::new(bin)
        .arg("reset")
        .env("ASKBAR_ADMIN_DATA_DIR", dir)
        .env("ASKBAR_ADMIN_TOKEN_DIR", dir.join("admin-token"))
        .env("ASKBAR_ADMIN_SECRETS_DIR", dir.join("secrets"))
        .env("ASKBAR_AUDIT_API_URL", auditor_url)
        .output()
        .unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_from_the_command_line_is_recorded_first_and_not_made_without_the_record() {
    let dir = std::env::temp_dir().join(format!("ab-reset-{}", &ulid::Ulid::new().to_string()[16..]));
    std::fs::create_dir_all(&dir).unwrap();
    askbar_bootstrap::install_for_tests(&dir);
    {
        // An admin that has already changed its password: no reset is pending.
        let db = askbar_admin::AdminDb::open(&dir.join("admin.db")).unwrap();
        askbar_admin::web::init_password(&db, common::PW).unwrap();
        db.conn().execute("UPDATE admin_credentials SET must_change = 0", []).unwrap();
    }
    let must_change = || {
        let db = askbar_admin::AdminDb::open(&dir.join("admin.db")).unwrap();
        db.conn().query_row("SELECT must_change FROM admin_credentials WHERE id = 1", [], |r| r.get::<_, i64>(0)).unwrap()
    };

    // The auditor is away: nothing is reset, and the sentence says why.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let d = dir.clone();
    let (ok, said) = tokio::task::spawn_blocking(move || run_reset(&d, &dead)).await.unwrap();
    assert!(!ok);
    assert!(said.contains("nothing was changed") && said.contains("not reset"), "{said}");
    assert_eq!(must_change(), 0, "the password was left as it was");

    // The auditor takes the record: the reset is made, and its end is recorded.
    let (url, seen) = stub(201).await;
    let d = dir.clone();
    let (ok, said) = tokio::task::spawn_blocking(move || run_reset(&d, &url)).await.unwrap();
    assert!(ok, "{said}");
    assert_eq!(must_change(), 1);
    let seen = seen.lock().unwrap();
    let ends: Vec<(&str, &str)> = seen.iter().map(|b| (b["category"].as_str().unwrap(), b["outcome"].as_str().unwrap())).collect();
    assert_eq!(ends, [("admin_reset", "requested"), ("admin_reset", "done")]);
    assert_eq!(seen[0]["detail"], json!({ "by": "command line" }), "the record says who, and nothing about the password");
    let _ = std::fs::remove_dir_all(&dir);
}
