//! A response body that is written by blocking code as it goes: an export that streams a tar through age without holding it in memory
//! (ADR 0013). The blocking task writes to a [`ChunkWriter`]; the other end of the channel is the body. When the client goes away the
//! next write fails, so the task stops and drops what it held (its work files), and an error sent last ends the connection badly, so
//! a reader knows the body is not whole.

use axum::body::{Body, Bytes};
use std::io::Write;
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub type Chunk = Result<Bytes, std::io::Error>;

/// Pieces of this size go to the client (the channel holds a few, so the task cannot run far ahead of a slow reader).
const CHUNK: usize = 64 * 1024;

/// A writer that sends what it is given to a response body, in chunks.
pub struct ChunkWriter {
    tx: Sender<Chunk>,
    buf: Vec<u8>,
}

impl ChunkWriter {
    fn send(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = Bytes::from(std::mem::take(&mut self.buf));
        self.tx.blocking_send(Ok(chunk)).map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the client went away"))
    }

    /// Tells the body the export did not finish: the connection ends with an error, not as if it were whole.
    pub fn fail(&self) {
        let _ = self.tx.blocking_send(Err(std::io::Error::other("the export did not finish")));
    }
}

impl Write for ChunkWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.send()
    }
}

/// A writer for the blocking task and the body that sends what it writes. The body has no length.
pub fn body() -> (ChunkWriter, Body) {
    let (tx, rx): (Sender<Chunk>, Receiver<Chunk>) = channel(4);
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    (ChunkWriter { tx, buf: Vec::new() }, Body::from_stream(stream))
}
