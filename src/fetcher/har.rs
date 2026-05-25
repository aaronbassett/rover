//! HAR (HTTP Archive) debug recorder.
//!
//! Activated by `[debug] har_path` in config. Wraps each fetch round-trip
//! into an `har::v1_2::Entries` entry and flushes periodically. Bodies are
//! truncated to `[debug] har_body_cap`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum HarError {
    #[error("could not open har file {path:?}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("could not serialize har: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("could not write har file {path:?}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Snapshot of one HTTP round-trip handed to the recorder. Keeps the
/// recorder ignorant of reqwest internals so it's easy to unit test.
#[derive(Debug, Clone)]
pub struct RecordedExchange {
    pub url: String,
    pub method: String,
    pub request_headers: Vec<(String, String)>,
    pub response_status: u16,
    pub response_headers: Vec<(String, String)>,
    pub response_body: Vec<u8>,
    pub duration: Duration,
}

/// HAR recorder. Holds an in-memory accumulator and flushes the full
/// file on `flush()`. For long-running servers, callers should call
/// `flush` on an interval; for short-lived CLI runs, calling once at
/// shutdown is sufficient.
#[derive(Debug, Clone)]
pub struct HarRecorder {
    path: PathBuf,
    body_cap: u64,
    entries: Arc<Mutex<Vec<har::v1_2::Entries>>>,
}

impl HarRecorder {
    pub fn new(path: PathBuf, body_cap: u64) -> Result<Self, HarError> {
        // Validate writability by creating + truncating the file.
        std::fs::File::create(&path).map_err(|source| HarError::Open {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            path,
            body_cap,
            entries: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub async fn record(&self, ex: RecordedExchange) -> Result<(), HarError> {
        let entry = self.build_entry(ex);
        self.entries.lock().await.push(entry);
        Ok(())
    }

    pub async fn flush(&self) -> Result<(), HarError> {
        let entries = self.entries.lock().await.clone();
        let har_doc = har::Har {
            log: har::Spec::V1_2(har::v1_2::Log {
                creator: har::v1_2::Creator {
                    name: "rover".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    comment: None,
                },
                browser: None,
                pages: None,
                entries,
                comment: None,
            }),
        };
        let json = serde_json::to_string_pretty(&har_doc)?;
        std::fs::write(&self.path, json).map_err(|source| HarError::Write {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    fn build_entry(&self, ex: RecordedExchange) -> har::v1_2::Entries {
        let cap = self.body_cap as usize;
        let (text, truncated) = if ex.response_body.len() > cap {
            (
                String::from_utf8_lossy(&ex.response_body[..cap]).into_owned(),
                true,
            )
        } else {
            (
                String::from_utf8_lossy(&ex.response_body).into_owned(),
                false,
            )
        };
        let comment =
            truncated.then(|| format!("truncated at {} bytes (har_body_cap)", self.body_cap));
        har::v1_2::Entries {
            pageref: None,
            started_date_time: jiff::Timestamp::now().to_string(),
            time: ex.duration.as_millis() as f64,
            request: har::v1_2::Request {
                method: ex.method,
                url: ex.url,
                http_version: "HTTP/1.1".to_string(),
                cookies: vec![],
                headers: ex
                    .request_headers
                    .into_iter()
                    .map(|(name, value)| har::v1_2::Headers {
                        name,
                        value,
                        comment: None,
                    })
                    .collect(),
                query_string: vec![],
                post_data: None,
                headers_size: -1,
                body_size: -1,
                comment: None,
            },
            response: har::v1_2::Response {
                status: i64::from(ex.response_status),
                status_text: String::new(),
                http_version: "HTTP/1.1".to_string(),
                cookies: vec![],
                headers: ex
                    .response_headers
                    .into_iter()
                    .map(|(name, value)| har::v1_2::Headers {
                        name,
                        value,
                        comment: None,
                    })
                    .collect(),
                content: har::v1_2::Content {
                    size: ex.response_body.len() as i64,
                    compression: None,
                    mime_type: None,
                    text: Some(text),
                    encoding: None,
                    comment,
                },
                redirect_url: None,
                headers_size: -1,
                body_size: -1,
                comment: None,
            },
            cache: har::v1_2::Cache::default(),
            timings: har::v1_2::Timings {
                blocked: Some(-1.0),
                dns: Some(-1.0),
                connect: Some(-1.0),
                send: 0.0,
                wait: ex.duration.as_millis() as f64,
                receive: 0.0,
                ssl: Some(-1.0),
                comment: None,
            },
            server_ip_address: None,
            connection: None,
            comment: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn recorder_writes_entry_on_record() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("rover.har");
        let recorder = HarRecorder::new(path.clone(), 64 * 1024).unwrap();
        recorder
            .record(RecordedExchange {
                url: "https://example.com/".to_string(),
                method: "GET".to_string(),
                request_headers: vec![("user-agent".into(), "Rover/0.1".into())],
                response_status: 200,
                response_headers: vec![("content-type".into(), "text/html".into())],
                response_body: b"<html></html>".to_vec(),
                duration: Duration::from_millis(50),
            })
            .await
            .unwrap();
        recorder.flush().await.unwrap();
        assert!(path.exists(), "har file should exist");
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["log"]["version"], "1.2");
        assert!(parsed["log"]["entries"].is_array());
        assert_eq!(parsed["log"]["entries"].as_array().unwrap().len(), 1);
        let entry = &parsed["log"]["entries"][0];
        assert_eq!(entry["request"]["url"], "https://example.com/");
        assert_eq!(entry["response"]["status"], 200);
    }

    #[tokio::test]
    async fn body_truncated_when_over_cap() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("trunc.har");
        let recorder = HarRecorder::new(path.clone(), 8).unwrap();
        recorder
            .record(RecordedExchange {
                url: "https://x/".to_string(),
                method: "GET".to_string(),
                request_headers: vec![],
                response_status: 200,
                response_headers: vec![],
                response_body: b"hello-this-body-is-large".to_vec(),
                duration: Duration::from_millis(5),
            })
            .await
            .unwrap();
        recorder.flush().await.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entry = &parsed["log"]["entries"][0];
        let body_text = entry["response"]["content"]["text"].as_str().unwrap_or("");
        assert!(
            body_text.len() <= 8,
            "body should be truncated to <= cap, was {}",
            body_text.len()
        );
        assert!(
            entry["response"]["content"]["comment"]
                .as_str()
                .unwrap_or("")
                .contains("truncated"),
            "expected truncation comment",
        );
    }

    #[test]
    fn new_rejects_unwritable_path() {
        let bad = std::path::PathBuf::from("/this/path/cannot/exist/rover.har");
        let r = HarRecorder::new(bad, 1024);
        assert!(r.is_err(), "expected error for unwritable path");
    }
}
