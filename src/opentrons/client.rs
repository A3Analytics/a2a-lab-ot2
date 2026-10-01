//! Typed HTTP client for a local Opentrons robot-server.

use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;
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
    pub fn new(base: &str) -> Result<Self, A2aLabError> {
        let base =
            Url::parse(base).map_err(|error| A2aLabError::invalid("url", error.to_string()))?;
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| transport(&error))?;
        Ok(Self { http, base })
    }

    /// `GET /health`.
    pub async fn health(&self) -> Result<Health, A2aLabError> {
        self.send_json(Method::GET, "health", None).await
    }

    /// `POST /protocols` with the bundled protocol file.
    pub async fn upload_protocol(
        &self,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Protocol, A2aLabError> {
        self.upload_bytes("protocols", "files", filename, bytes, "text/x-python")
            .await
    }

    /// Multipart POST used by protocol, data-file, and Wi-Fi key uploads.
    pub async fn upload_bytes<T>(
        &self,
        path: &str,
        field: &str,
        filename: &str,
        bytes: Vec<u8>,
        mime: &str,
    ) -> Result<T, A2aLabError>
    where
        T: serde::de::DeserializeOwned,
    {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_owned())
            .mime_str(mime)
            .map_err(|error| A2aLabError::invalid("file", error.to_string()))?;
        let form = reqwest::multipart::Form::new().part(field.to_owned(), part);
        let response = self
            .http
            .post(self.url(path)?)
            .header(VERSION_HEADER, VERSION)
            .multipart(form)
            .send()
            .await
            .map_err(|error| transport(&error))?;
        self.read_data(response).await
    }

    /// `GET /protocols/{id}`.
    pub async fn protocol(&self, protocol_id: &str) -> Result<Protocol, A2aLabError> {
        self.send_json(Method::GET, &format!("protocols/{protocol_id}"), None)
            .await
    }

    /// Polls until the latest analysis is no longer pending.
    pub async fn wait_for_analysis(&self, protocol_id: &str) -> Result<Protocol, A2aLabError> {
        for _ in 0..60 {
            let protocol = self.protocol(protocol_id).await?;
            if !protocol.analysis_summaries.is_empty()
                && protocol.analysis_summaries.iter().all(analysis_ready)
            {
                return Ok(protocol);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(A2aLabError::unavailable(
            "protocol analysis did not finish in time",
        ))
    }

    /// `POST /runs`. Dismisses any current run first; robot-server allows only one.
    pub async fn create_run(&self, protocol_id: &str) -> Result<Run, A2aLabError> {
        self.release_current().await?;
        self.send_json(
            Method::POST,
            "runs",
            Some(json!({ "data": { "protocolId": protocol_id } })),
        )
        .await
    }

    /// `GET /runs/{id}`.
    pub async fn run(&self, run_id: &str) -> Result<Run, A2aLabError> {
        self.send_json(Method::GET, &format!("runs/{run_id}"), None)
            .await
    }

    /// `GET /runs`.
    pub async fn runs(&self) -> Result<Vec<Run>, A2aLabError> {
        self.send_json(Method::GET, "runs?pageLength=50", None)
            .await
    }

    /// `DELETE /runs/{id}`.
    pub async fn delete_run(&self, run_id: &str) -> Result<(), A2aLabError> {
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
    pub async fn run_action(&self, run_id: &str, action_type: &str) -> Result<(), A2aLabError> {
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
    pub async fn run_commands(&self, run_id: &str) -> Result<Vec<Command>, A2aLabError> {
        self.send_json(
            Method::GET,
            &format!("runs/{run_id}/commands?pageLength=100"),
            None,
        )
        .await
    }

    /// `GET /logs/{identifier}?format=json`. Missing or empty journals yield an empty string.
    pub async fn troubleshooting_log(&self, identifier: &str) -> Result<String, A2aLabError> {
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
    ) -> Result<Command, A2aLabError> {
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

    /// Calls an arbitrary robot-server path and returns JSON (`data` unwrapped when present).
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, A2aLabError> {
        let method = parse_method(method)?;
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .header(VERSION_HEADER, VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self.send_raw(request).await?;
        let status = response.status();
        let text = response.text().await.map_err(|error| transport(&error))?;
        if !status.is_success() {
            return Err(status_error(status, &text));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        if let Ok(envelope) = serde_json::from_str::<Envelope<Value>>(&text) {
            return Ok(envelope.data);
        }
        serde_json::from_str(&text).or(Ok(Value::String(text)))
    }

    async fn release_current(&self) -> Result<(), A2aLabError> {
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
    ) -> Result<T, A2aLabError>
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
    ) -> Result<reqwest::Response, A2aLabError> {
        let retry = request.try_clone();
        match request.send().await {
            Ok(response) => Ok(response),
            Err(first) => match retry {
                Some(retry) => retry.send().await.map_err(|error| transport(&error)),
                None => Err(transport(&first)),
            },
        }
    }

    async fn read_data<T>(&self, response: reqwest::Response) -> Result<T, A2aLabError>
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
        serde_json::from_str(&body).map_err(|error| A2aLabError::protocol(error.to_string()))
    }

    fn url(&self, path: &str) -> Result<Url, A2aLabError> {
        let mut base = self.base.as_str().trim_end_matches('/').to_owned();
        base.push('/');
        base.push_str(path.trim_start_matches('/'));
        Url::parse(&base).map_err(|error| A2aLabError::invalid("url", error.to_string()))
    }
}

fn transport(error: &reqwest::Error) -> A2aLabError {
    A2aLabError::unavailable(format!(
        "robot-server is unreachable ({error}); keep `mise run ot2-simulator` running"
    ))
}

fn analysis_ready(summary: &AnalysisSummary) -> bool {
    summary.status != "pending"
}

fn parse_method(method: &str) -> Result<Method, A2aLabError> {
    match method {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "PATCH" => Ok(Method::PATCH),
        "DELETE" => Ok(Method::DELETE),
        _ => Err(A2aLabError::invalid("method", method.to_owned())),
    }
}

fn status_error(status: StatusCode, body: &str) -> A2aLabError {
    let detail = error_detail(body);
    if status == StatusCode::NOT_FOUND {
        A2aLabError::not_found("resource", detail)
    } else if status.is_client_error() {
        A2aLabError::invalid("request", detail)
    } else {
        A2aLabError::unavailable(detail)
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
