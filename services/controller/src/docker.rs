//! A very small Docker Engine API client over the unix socket: list the containers of one Compose
//! project and start, stop or restart one of them. These are the only calls the controller can make.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_BODY: usize = 8 * 1024 * 1024;

pub const PROJECT_LABEL: &str = "com.docker.compose.project";
pub const SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Debug)]
pub enum DockerError {
    /// The socket did not answer.
    Unreachable(String),
    /// Docker answered with this status and message.
    Refused(u16, String),
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::Unreachable(e) => write!(f, "Docker did not answer: {e}"),
            DockerError::Refused(s, m) => write!(f, "Docker answered {s}: {m}"),
        }
    }
}

#[derive(Clone)]
pub struct Docker {
    socket: PathBuf,
}

/// One container, as far as the controller cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub service: String,
    /// `running`, `exited`, `restarting`, `paused`, `created`, ...
    pub state: String,
    /// Docker's own sentence, such as `Up 3 hours (healthy)`.
    pub status: String,
}

impl Container {
    /// `healthy`, `unhealthy` or `starting` when the image has a health check.
    pub fn health(&self) -> Option<&'static str> {
        let s = &self.status;
        if s.contains("(unhealthy)") {
            Some("unhealthy")
        } else if s.contains("(healthy)") {
            Some("healthy")
        } else if s.contains("(health: starting)") {
            Some("starting")
        } else {
            None
        }
    }
}

fn decode_chunked(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = b.windows(2).position(|w| w == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&b[..line_end]).ok()?.split(';').next()?.trim(), 16).ok()?;
        b = &b[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

impl Docker {
    pub fn new(socket: impl AsRef<Path>) -> Docker {
        Docker { socket: socket.as_ref().to_path_buf() }
    }

    async fn request(&self, method: &str, path: &str) -> Result<(u16, Vec<u8>), DockerError> {
        let unreachable = |e: &dyn std::fmt::Display| DockerError::Unreachable(e.to_string());
        let run = async {
            let mut s = UnixStream::connect(&self.socket).await.map_err(|e| unreachable(&e))?;
            let req = format!("{method} {path} HTTP/1.1\r\nHost: docker\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
            s.write_all(req.as_bytes()).await.map_err(|e| unreachable(&e))?;
            let mut raw = Vec::new();
            (&mut s).take(MAX_BODY as u64).read_to_end(&mut raw).await.map_err(|e| unreachable(&e))?;
            let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| unreachable(&"no response"))?;
            let head = String::from_utf8_lossy(&raw[..split]).to_string();
            let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| unreachable(&"bad response"))?;
            let body = &raw[split + 4..];
            let chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
            let body = if chunked { decode_chunked(body).ok_or_else(|| unreachable(&"bad chunking"))? } else { body.to_vec() };
            Ok((status, body))
        };
        tokio::time::timeout(TIMEOUT, run).await.map_err(|_| DockerError::Unreachable("timed out".into()))?
    }

    fn message(body: &[u8]) -> String {
        serde_json::from_slice::<Value>(body).ok().and_then(|v| v["message"].as_str().map(String::from)).unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect())
    }

    /// The Compose project this container belongs to, read from its own labels (`id` is its hostname by default).
    pub async fn project_of(&self, id: &str) -> Result<String, DockerError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return Err(DockerError::Refused(400, "not a container id".into()));
        }
        let (status, body) = self.request("GET", &format!("/containers/{id}/json")).await?;
        if status != 200 {
            return Err(DockerError::Refused(status, Self::message(&body)));
        }
        let v: Value = serde_json::from_slice(&body).map_err(|e| DockerError::Unreachable(e.to_string()))?;
        v["Config"]["Labels"][PROJECT_LABEL].as_str().map(String::from).ok_or_else(|| DockerError::Refused(404, "this container has no Compose project label".into()))
    }

    /// Every container (running or not) of `project`, with its service name.
    pub async fn project_containers(&self, project: &str) -> Result<Vec<Container>, DockerError> {
        let filters = serde_json::json!({ "label": [format!("{PROJECT_LABEL}={project}")] }).to_string();
        let q: String = filters.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        let (status, body) = self.request("GET", &format!("/containers/json?all=1&filters={q}")).await?;
        if status != 200 {
            return Err(DockerError::Refused(status, Self::message(&body)));
        }
        let v: Value = serde_json::from_slice(&body).map_err(|e| DockerError::Unreachable(e.to_string()))?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        Some(Container {
                            id: c["Id"].as_str()?.to_string(),
                            service: c["Labels"][SERVICE_LABEL].as_str()?.to_string(),
                            state: c["State"].as_str().unwrap_or("").to_string(),
                            status: c["Status"].as_str().unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `start`, `stop` or `restart` for the container with this id (a 64-hex id from `project_containers`).
    /// Already in the wanted state (304) counts as done.
    pub async fn act(&self, id: &str, action: Action) -> Result<(), DockerError> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DockerError::Refused(400, "not a container id".into()));
        }
        let path = match action {
            Action::Start => format!("/containers/{id}/start"),
            Action::Stop => format!("/containers/{id}/stop?t=10"),
            Action::Restart => format!("/containers/{id}/restart?t=10"),
        };
        let (status, body) = self.request("POST", &path).await?;
        match status {
            204 | 304 => Ok(()),
            s => Err(DockerError::Refused(s, Self::message(&body))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "start" => Some(Action::Start),
            "stop" => Some(Action::Stop),
            "restart" => Some(Action::Restart),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_bodies_decode() {
        assert_eq!(decode_chunked(b"5\r\nhello\r\n3\r\n!!!\r\n0\r\n\r\n").unwrap(), b"hello!!!");
        assert!(decode_chunked(b"zz\r\n").is_none());
    }

    #[test]
    fn health_comes_from_the_status_sentence() {
        let c = |s: &str| Container { id: String::new(), service: String::new(), state: "running".into(), status: s.into() };
        assert_eq!(c("Up 3 hours (healthy)").health(), Some("healthy"));
        assert_eq!(c("Up 3 hours (unhealthy)").health(), Some("unhealthy"));
        assert_eq!(c("Up 2 seconds (health: starting)").health(), Some("starting"));
        assert_eq!(c("Up 3 hours").health(), None);
    }

    #[test]
    fn only_the_three_actions_parse() {
        assert_eq!(Action::parse("restart"), Some(Action::Restart));
        assert_eq!(Action::parse("kill"), None);
        assert_eq!(Action::parse("exec"), None);
    }
}
