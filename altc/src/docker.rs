//! Reads a session container's log stream to pick up the runner's progress.
//! The runner prints `ALTC-PROGRESS:<pct>` (and `:FAILED:<reason>`); nothing
//! else in the stream is forwarded.

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use std::path::PathBuf;

#[derive(Clone)]
pub struct DockerClient {
    socket: PathBuf,
    http: Client<UnixConnector, Full<Bytes>>,
}

pub enum RunnerUpdate {
    Percent(u8),
    Failed(String),
}

impl DockerClient {
    pub fn from_env() -> Self {
        let socket =
            std::env::var("WOLF_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".into());
        Self {
            socket: socket.into(),
            http: Client::unix(),
        }
    }

    /// Follows a container's logs, invoking `on_update` for each progress line.
    pub async fn follow_progress<F>(&self, container_id: &str, mut on_update: F)
    where
        F: FnMut(RunnerUpdate),
    {
        let path = format!("/containers/{container_id}/logs?follow=1&stdout=1&stderr=1&tail=0");
        let uri: hyper::Uri = Uri::new(&self.socket, &path).into();
        let Ok(req) = hyper::Request::get(uri).body(Full::new(Bytes::new())) else {
            return;
        };
        let Ok(res) = self.http.request(req).await else {
            tracing::debug!("cannot follow logs for {container_id}");
            return;
        };

        let mut body = res.into_body();
        let mut buf: Vec<u8> = Vec::new();
        while let Some(Ok(frame)) = body.frame().await {
            let Some(chunk) = frame.data_ref() else {
                continue;
            };
            buf.extend_from_slice(chunk);
            // Docker multiplexes with an 8-byte header per frame; the payload is
            // plain text, so scanning for lines is enough to find our markers.
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line);
                if let Some(update) = parse_progress(&text) {
                    on_update(update);
                }
            }
            if buf.len() > 64 * 1024 {
                buf.clear();
            }
        }
    }
}

fn parse_progress(line: &str) -> Option<RunnerUpdate> {
    let idx = line.find("ALTC-PROGRESS:")?;
    let rest = line[idx + "ALTC-PROGRESS:".len()..].trim();
    let mut parts = rest.splitn(2, ':');
    let pct: u8 = parts.next()?.trim().parse().ok()?;
    match parts.next() {
        Some(extra) if extra.contains("FAILED") => Some(RunnerUpdate::Failed(
            extra.trim_start_matches("FAILED:").trim().to_string(),
        )),
        _ => Some(RunnerUpdate::Percent(pct.min(100))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_percent_and_failure() {
        assert!(matches!(
            parse_progress("ALTC-PROGRESS:45"),
            Some(RunnerUpdate::Percent(45))
        ));
        assert!(matches!(
            parse_progress("\u{1}\u{0}junk ALTC-PROGRESS:100\n"),
            Some(RunnerUpdate::Percent(100))
        ));
        match parse_progress("ALTC-PROGRESS:0:FAILED:game files not found") {
            Some(RunnerUpdate::Failed(r)) => assert_eq!(r, "game files not found"),
            _ => panic!("expected failure"),
        }
        assert!(parse_progress("nothing here").is_none());
    }
}
