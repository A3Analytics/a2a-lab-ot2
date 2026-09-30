//! Typed HTTP client for a local Opentrons robot-server.

use std::sync::OnceLock;
use std::time::Duration;

use a2a_lab_sdk::SdkError;
use reqwest::{Method, StatusCode, Url};
use serde_json::{Value, json};

use super::model::{AnalysisSummary, Command, Envelope, Health, Protocol, Run};

const VERSION_HEADER: &str = "Opentrons-Version";
const VERSION: &str = "*";

/// HTTP client for robot-server endpoints used by this example.
#[derive(Clone)]
pub struct OpentronsClient {
    http: reqwest::Client,
    base: Url,
}

impl OpentronsClient {
    /// Creates a client for `base`, such as `http://127.0.0.1:31950`.
    pub fn new(base: &str) -> Result<Self, SdkError> {
        let base = Url::parse(base).map_err(|error| SdkError::invalid("url", error.to_string()))?;
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| transport(&error))?;
        Ok(Self { http, base })
    }

    /// `GET /health`.
    pub async fn health(&self) -> Result<Health, SdkError> {
        self.send_json(Method::GET, "health", None).await
    }

    /// `POST /protocols` with the bundled protocol file.
    pub async fn upload_protocol(
        &self,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Protocol, SdkError> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_owned())
            .mime_str("text/x-python")
            .map_err(|error| SdkError::invalid("protocol", error.to_string()))?;
        let form = reqwest::multipart::Form::new().part("files", part);
        let response = self
            .http
            .post(self.url("protocols")?)
            .header(VERSION_HEADER, VERSION)
            .multipart(form)
            .send()
            .await
            .map_err(|error| transport(&error))?;
        self.read_data(response).await
    }

    /// `GET /protocols/{id}`.
    pub async fn protocol(&self, protocol_id: &str) -> Result<Protocol, SdkError> {
        self.send_json(Method::GET, &format!("protocols/{protocol_id}"), None)
            .await
    }

    /// Polls until the latest analysis is no longer pending.
    pub async fn wait_for_analysis(&self, protocol_id: &str) -> Result<Protocol, SdkError> {
        for _ in 0..60 {
            let protocol = self.protocol(protocol_id).await?;
            if !protocol.analysis_summaries.is_empty()
                && protocol.analysis_summaries.iter().all(analysis_ready)
            {
                return Ok(protocol);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(SdkError::unavailable(
            "protocol analysis did not finish in time",
        ))
    }

    /// `POST /runs`. Dismisses any current run first; robot-server allows only one.
    pub async fn create_run(&self, protocol_id: &str) -> Result<Run, SdkError> {
        self.release_current().await?;
        self.send_json(
            Method::POST,
            "runs",
            Some(json!({ "data": { "protocolId": protocol_id } })),
        )
        .await
    }

    /// `GET /runs/{id}`.
    pub async fn run(&self, run_id: &str) -> Result<Run, SdkError> {
        self.send_json(Method::GET, &format!("runs/{run_id}"), None)
            .await
    }

    /// `GET /runs`.
    pub async fn runs(&self) -> Result<Vec<Run>, SdkError> {
        self.send_json(Method::GET, "runs?pageLength=50", None)
            .await
    }

    /// `DELETE /runs/{id}`.
    pub async fn delete_run(&self, run_id: &str) -> Result<(), SdkError> {
        let response = self
            .send_raw(
                self.http
                    .delete(self.url(&format!("runs/{run_id}"))?)
                    .header(VERSION_HEADER, VERSION),
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(
                response.status(),
                &response.text().await.unwrap_or_default(),
            ))
        }
    }

    /// `POST /runs/{id}/actions`.
    pub async fn run_action(&self, run_id: &str, action_type: &str) -> Result<(), SdkError> {
        let _: Value = self
            .send_json(
                Method::POST,
                &format!("runs/{run_id}/actions"),
                Some(json!({ "data": { "actionType": action_type } })),
            )
            .await?;
        Ok(())
    }

    /// `GET /runs/{id}/commands`.
    pub async fn run_commands(&self, run_id: &str) -> Result<Vec<Command>, SdkError> {
        self.send_json(
            Method::GET,
            &format!("runs/{run_id}/commands?pageLength=100"),
            None,
        )
        .await
    }

    /// `GET /logs/{identifier}?format=json`. Missing or empty journals yield an empty string.
    ///
    /// Skips the HTTP call when `journalctl` is not on PATH. On macOS the
    /// simulator's `/logs` handler otherwise crashes with `FileNotFoundError`.
    pub async fn troubleshooting_log(&self, identifier: &str) -> Result<String, SdkError> {
        if !journalctl_available() {
            return Ok(String::new());
        }
        let Ok(response) = self
            .send_raw(
                self.http
                    .get(self.url(&format!("logs/{identifier}?format=json&records=100"))?)
                    .header(VERSION_HEADER, VERSION),
            )
            .await
        else {
            return Ok(String::new());
        };
        if !response.status().is_success() {
            return Ok(String::new());
        }
        Ok(response.text().await.unwrap_or_default())
    }

    /// `POST /commands?waitUntilComplete=true`.
    pub async fn execute_command(
        &self,
        command_type: &str,
        params: Value,
    ) -> Result<Command, SdkError> {
        self.send_json(
            Method::POST,
            "commands?waitUntilComplete=true",
            Some(json!({
                "data": {
                    "commandType": command_type,
                    "params": params
                }
            })),
        )
        .await
    }

    async fn release_current(&self) -> Result<(), SdkError> {
        for run in self.runs().await? {
            if !run.current {
                continue;
            }
            if matches!(
                run.status.as_str(),
                "running"
                    | "paused"
                    | "pause-requested"
                    | "blocked-by-open-door"
                    | "awaiting-recovery"
            ) {
                let _ = self.run_action(&run.id, "stop").await;
            }
            let _: Run = self
                .send_json(
                    Method::PATCH,
                    &format!("runs/{}", run.id),
                    Some(json!({ "data": { "current": false } })),
                )
                .await?;
        }
        Ok(())
    }

    async fn send_json<T>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, SdkError>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .header(VERSION_HEADER, VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self.send_raw(request).await?;
        self.read_data(response).await
    }

    async fn send_raw(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, SdkError> {
        let retry = request.try_clone();
        match request.send().await {
            Ok(response) => Ok(response),
            Err(first) => match retry {
                Some(retry) => retry.send().await.map_err(|error| transport(&error)),
                None => Err(transport(&first)),
            },
        }
    }

    async fn read_data<T>(&self, response: reqwest::Response) -> Result<T, SdkError>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();
        let body = response.text().await.map_err(|error| transport(&error))?;
        if !status.is_success() {
            return Err(status_error(status, &body));
        }
        if let Ok(envelope) = serde_json::from_str::<Envelope<T>>(&body) {
            return Ok(envelope.data);
        }
        serde_json::from_str(&body).map_err(|error| SdkError::protocol(error.to_string()))
    }

    fn url(&self, path: &str) -> Result<Url, SdkError> {
        let mut base = self.base.as_str().trim_end_matches('/').to_owned();
        base.push('/');
        base.push_str(path.trim_start_matches('/'));
        Url::parse(&base).map_err(|error| SdkError::invalid("url", error.to_string()))
    }
}

fn journalctl_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .any(|dir| dir.join("journalctl").is_file())
    })
}

fn transport(error: &reqwest::Error) -> SdkError {
    SdkError::unavailable(format!(
        "robot-server is unreachable ({error}); keep `mise run start` running"
    ))
}

fn analysis_ready(summary: &AnalysisSummary) -> bool {
    summary.status != "pending"
}

fn status_error(status: StatusCode, body: &str) -> SdkError {
    let detail = error_detail(body);
    if status == StatusCode::NOT_FOUND {
        SdkError::not_found("resource", detail)
    } else if status.is_client_error() {
        SdkError::invalid("request", detail)
    } else {
        SdkError::unavailable(detail)
    }
}

fn error_detail(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    parsed
        .as_ref()
        .and_then(|value| value.get("errors"))
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
        .and_then(|error| {
            error
                .get("detail")
                .or_else(|| error.get("title"))
                .and_then(Value::as_str)
        })
        .map_or_else(|| body.chars().take(200).collect(), ToOwned::to_owned)
}
