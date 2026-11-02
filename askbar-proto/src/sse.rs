//! Incremental Server-Sent Events parsing, shared by the server (provider streams) and the client (chat stream).

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental Server-Sent Events parser. Accepts arbitrary byte chunks, so
/// lines and multi-byte characters may be split anywhere.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // \n
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&String::from_utf8_lossy(&line), &mut out);
        }
        out
    }

    /// Flush a final event if the stream ended without a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let text = String::from_utf8_lossy(&rest).trim_end_matches('\r').to_string();
            self.line(&text, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if line.starts_with(':') {
            // comment / keep-alive
        } else {
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data.is_empty() && self.event.is_none() {
            return;
        }
        out.push(SseEvent { event: self.event.take(), data: self.data.join("\n") });
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_handles_split_chunks_crlf_comments_and_utf8() {
        let raw = ": ping\r\nevent: x\r\ndata: {\"a\":\"é\"}\r\n\r\ndata: one\ndata: two\n\n";
        let bytes = raw.as_bytes();
        // Feed one byte at a time: the harshest possible chunking.
        let mut p = SseParser::default();
        let mut events = Vec::new();
        for b in bytes {
            events.extend(p.feed(&[*b]));
        }
        events.extend(p.finish());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("x"));
        assert_eq!(events[0].data, "{\"a\":\"é\"}");
        assert_eq!(events[1].data, "one\ntwo");
    }

    #[test]
    fn sse_finish_flushes_unterminated_event() {
        let mut p = SseParser::default();
        assert!(p.feed(b"data: tail").is_empty());
        let ev = p.finish();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "tail");
    }
}
