//! Generations: chat turns that run as server tasks and outlive the client that started them.
//!
//! This module holds the in-memory registry: each running generation has a `Live` that buffers
//! its deltas so a client can attach, drop and attach again without losing or repeating text.

use futures_util::Stream;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// What a subscriber sees. `seq` counts deltas from 1, and is what SSE `id:` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenEvent {
    Delta { seq: u64, text: String },
    Done,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    Done,
    Error(String),
}

#[derive(Default)]
struct Inner {
    deltas: Vec<String>,
    terminal: Option<Terminal>,
}

/// One running generation's buffered output.
#[derive(Default)]
pub struct Live {
    inner: Mutex<Inner>,
    notify: Notify,
}

impl Live {
    pub fn new() -> Arc<Live> {
        Arc::new(Live::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends a delta. Ignored once the generation has finished.
    pub fn push(&self, text: &str) {
        {
            let mut g = self.lock();
            if g.terminal.is_some() {
                return;
            }
            g.deltas.push(text.to_string());
        }
        self.notify.notify_waiters();
    }

    /// Ends the generation; the first call wins.
    pub fn finish(&self, terminal: Terminal) {
        {
            let mut g = self.lock();
            if g.terminal.is_some() {
                return;
            }
            g.terminal = Some(terminal);
        }
        self.notify.notify_waiters();
    }

    pub fn delta_count(&self) -> u64 {
        self.lock().deltas.len() as u64
    }

    pub fn is_finished(&self) -> bool {
        self.lock().terminal.is_some()
    }

    /// Deltas after the first `after` of them, then live ones, then the end. Starting after
    /// the end of a finished generation yields just the terminal event.
    pub fn subscribe(self: &Arc<Self>, after: u64) -> impl Stream<Item = GenEvent> + Send + 'static {
        struct State {
            live: Arc<Live>,
            next: u64,
            ended: bool,
        }
        futures_util::stream::unfold(State { live: self.clone(), next: after, ended: false }, |mut s| async move {
            if s.ended {
                return None;
            }
            let live = s.live.clone();
            loop {
                // Register for wake-ups before looking, so a push between the look and the wait is not missed.
                let notified = live.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let step = {
                    let g = live.lock();
                    if (s.next as usize) < g.deltas.len() {
                        Some(GenEvent::Delta { seq: s.next + 1, text: g.deltas[s.next as usize].clone() })
                    } else {
                        g.terminal.as_ref().map(|t| match t {
                            Terminal::Done => GenEvent::Done,
                            Terminal::Error(m) => GenEvent::Error { message: m.clone() },
                        })
                    }
                };
                match step {
                    Some(ev @ GenEvent::Delta { .. }) => {
                        s.next += 1;
                        return Some((ev, s));
                    }
                    Some(ev) => {
                        s.ended = true;
                        return Some((ev, s));
                    }
                    None => notified.await,
                }
            }
        })
    }
}

/// The running generations, by id.
#[derive(Default)]
pub struct Generations {
    live: Mutex<HashMap<String, Arc<Live>>>,
}

impl Generations {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Live>>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, id: &str, live: Arc<Live>) {
        self.map().insert(id.to_string(), live);
    }

    pub fn get(&self, id: &str) -> Option<Arc<Live>> {
        self.map().get(id).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.map().remove(id);
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::time::Duration;

    fn d(seq: u64, t: &str) -> GenEvent {
        GenEvent::Delta { seq, text: t.into() }
    }

    async fn collect(s: impl Stream<Item = GenEvent>) -> Vec<GenEvent> {
        tokio::time::timeout(Duration::from_secs(5), s.collect::<Vec<_>>()).await.expect("stream should end")
    }

    #[tokio::test]
    async fn replays_buffered_deltas_then_ends() {
        let live = Live::new();
        live.push("a");
        live.push("b");
        live.finish(Terminal::Done);
        assert_eq!(collect(live.subscribe(0)).await, [d(1, "a"), d(2, "b"), GenEvent::Done]);
        assert_eq!(collect(live.subscribe(1)).await, [d(2, "b"), GenEvent::Done], "after=1 skips the first");
    }

    #[tokio::test]
    async fn follows_live_deltas_until_the_end() {
        let live = Live::new();
        live.push("a");
        let sub = tokio::spawn(collect(live.subscribe(0)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        live.push("b");
        tokio::time::sleep(Duration::from_millis(30)).await;
        live.push("c");
        live.finish(Terminal::Error("boom".into()));
        assert_eq!(sub.await.unwrap(), [d(1, "a"), d(2, "b"), d(3, "c"), GenEvent::Error { message: "boom".into() }]);
    }

    #[tokio::test]
    async fn a_late_subscriber_after_finish_gets_everything_once() {
        let live = Live::new();
        for t in ["x", "y", "z"] {
            live.push(t);
        }
        live.finish(Terminal::Done);
        live.push("ignored"); // nothing is added after the end
        live.finish(Terminal::Error("ignored".into())); // and the first ending wins
        assert!(live.is_finished());
        assert_eq!(live.delta_count(), 3);
        assert_eq!(collect(live.subscribe(0)).await, [d(1, "x"), d(2, "y"), d(3, "z"), GenEvent::Done]);
        assert_eq!(collect(live.subscribe(3)).await, [GenEvent::Done], "nothing left but the end");
    }

    #[tokio::test]
    async fn two_subscribers_each_see_every_delta() {
        let live = Live::new();
        let a = tokio::spawn(collect(live.subscribe(0)));
        live.push("1");
        let b = tokio::spawn(collect(live.subscribe(0)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        live.push("2");
        live.finish(Terminal::Done);
        let want = [d(1, "1"), d(2, "2"), GenEvent::Done];
        assert_eq!(a.await.unwrap(), want);
        assert_eq!(b.await.unwrap(), want);
    }

    #[tokio::test]
    async fn registry_inserts_gets_and_removes() {
        let reg = Generations::default();
        assert!(reg.get("g1").is_none() && reg.is_empty());
        let live = Live::new();
        reg.insert("g1", live.clone());
        assert!(Arc::ptr_eq(&reg.get("g1").unwrap(), &live));
        assert_eq!(reg.len(), 1);
        reg.remove("g1");
        assert!(reg.get("g1").is_none());
    }
}
