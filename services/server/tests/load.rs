//! Three replies at once, and what the stack adds before the first token (ADR 0002: under 200 ms at the 95th percentile, the
//! provider excluded; ADR 0010 rule 4; plan step 7.2, D31). This is a gate.
//!
//! A reply here takes the stack's own path: the server's generation code, a real model-gateway (in this process) that opens its
//! span with a real auditor and waits for the acknowledgment before it calls the provider, and the mock model
//! (`tests/e2e/mock_llm.py`). Each round starts 3 replies at once and times each from the send to its first text. The mock's own
//! time to its first text, measured the same way (3 at once, just before) and as the median of the 3, is subtracted from each,
//! so what is left is what Askbar added. The first rounds are not counted (connections and caches warm up).
//!
//! It fails only if the 95th percentile of the added time is over 200 ms in two runs in a row: one slow run on a busy machine is
//! noise, two are not. Run it with `cargo test -p askbar-server --test load -- --nocapture` to see the figures.

use futures_util::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONCURRENT: usize = 3;
const WARM_UP_ROUNDS: usize = 3;
const ROUNDS: usize = 20;
const LIMIT: Duration = Duration::from_millis(200);
const RUNS: usize = 2;

struct Mock(std::process::Child);
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_mock() -> (Mock, u16) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, "0"]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    // Port 0: the mock binds a free port itself and names it on this first line, printed only once it is bound.
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches("/v1").parse::<u16>().ok())
        .unwrap_or_else(|| panic!("the mock did not say where it listens: {line:?}"));
    // Keep reading: the mock logs every request, and a closed pipe would break it.
    std::thread::spawn(move || for _ in out.lines() {});
    (Mock(child), port)
}

/// The mock model's own time to its first piece of text, asked directly the way the gateway asks it.
async fn mock_time_to_first_text(http: &reqwest::Client, port: u16) -> Duration {
    let started = Instant::now();
    let mut resp = http
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .bearer_auth("sk-test")
        .json(&serde_json::json!({ "model": "m", "stream": true, "messages": [{ "role": "user", "content": "hello" }] }))
        .send()
        .await
        .expect("the mock answers");
    let mut first = None;
    while let Some(chunk) = resp.chunk().await.expect("the mock streams") {
        if first.is_none() && String::from_utf8_lossy(&chunk).contains("\"content\"") {
            first = Some(started.elapsed());
        }
    }
    first.expect("the mock sent text")
}

/// One reply through the stack, to its end; the time from the send to its first text.
async fn reply_time_to_first_text(state: Arc<askbar_server::AppState>, user: String, client: String, prompt: String) -> Duration {
    let started = Instant::now();
    let sub = askbar_server::generation::submit(
        &state,
        askbar_server::generation::Submit {
            space: askbar_server::testing::personal_space(&state, &user),
            user_id: user,
            client_id: client,
            conversation_id: None,
            provider_id: "p1".into(),
            model: "m".into(),
            prompt,
            client_request_id: None,
        },
    )
    .expect("the reply starts");
    let mut first = None;
    let mut ended_well = false;
    let mut events = Box::pin(sub.live.subscribe(0));
    while let Some(ev) = events.next().await {
        match ev {
            askbar_server::generation::GenEvent::Delta { .. } if first.is_none() => first = Some(started.elapsed()),
            askbar_server::generation::GenEvent::Done => ended_well = true,
            askbar_server::generation::GenEvent::Error { message, .. } => panic!("a reply failed: {message}"),
            _ => {}
        }
    }
    assert!(ended_well, "the reply did not finish");
    first.expect("the reply had text")
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((p / 100.0 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1]
}

/// What was added, by this run's replies: every reply's time to first text less the mock's own for its round.
struct Run {
    added: Vec<Duration>,
    mock: Vec<Duration>,
}

async fn one_run(state: &Arc<askbar_server::AppState>, user: &str, client: &str, port: u16, rounds: usize, counted_from: usize) -> Run {
    let http = reqwest::Client::new();
    let mut added = Vec::new();
    let mut mock_times = Vec::new();
    for round in 0..rounds {
        // The mock alone, 3 at once, then the stack's 3 at once.
        let mut mock: Vec<Duration> = futures_util::future::join_all((0..CONCURRENT).map(|_| mock_time_to_first_text(&http, port))).await;
        mock.sort();
        let mock_median = mock[CONCURRENT / 2];
        let replies = (0..CONCURRENT).map(|i| {
            let (state, user, client) = (state.clone(), user.to_string(), client.to_string());
            tokio::spawn(reply_time_to_first_text(state, user, client, format!("load {round}.{i}")))
        });
        for r in futures_util::future::join_all(replies).await {
            let took = r.expect("the reply task ran");
            if round >= counted_from {
                added.push(took.saturating_sub(mock_median));
            }
        }
        if round >= counted_from {
            mock_times.extend(mock);
        }
    }
    added.sort();
    mock_times.sort();
    Run { added, mock: mock_times }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_replies_at_once_add_under_200_ms_at_the_95th_percentile_before_the_first_token() {
    let (_mock, port) = start_mock();
    let (state, _dir, _rig) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{port}/v1")).await;
    let (user, client) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        (user, client)
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut p95s = Vec::new();
    for attempt in 1..=RUNS {
        let run = one_run(&state, &user, &client, port, WARM_UP_ROUNDS + ROUNDS, WARM_UP_ROUNDS).await;
        let (p50, p95, max) = (percentile(&run.added, 50.0), percentile(&run.added, 95.0), *run.added.last().unwrap());
        println!(
            "run {attempt}: {} replies, {CONCURRENT} at a time. Added before the first token: p50 {:.1} ms, p95 {:.1} ms (limit {} ms), max {:.1} ms. The mock's own: p50 {:.1} ms, p95 {:.1} ms.",
            run.added.len(),
            ms(p50),
            ms(p95),
            LIMIT.as_millis(),
            ms(max),
            ms(percentile(&run.mock, 50.0)),
            ms(percentile(&run.mock, 95.0)),
        );
        assert_eq!(run.added.len(), ROUNDS * CONCURRENT, "every counted reply finished");
        if p95 <= LIMIT {
            return;
        }
        p95s.push(p95);
    }
    panic!("the 95th percentile of the added time was over {} ms in {RUNS} runs: {p95s:?}", LIMIT.as_millis());
}
