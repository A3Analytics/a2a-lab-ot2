//! Read-only video from a configured HTTP archive.

use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, DEFAULT_MAX_IMAGE_BYTES, GetVideoClipRequest, GetVideoFrameAtRequest,
    GetVideoFrameAtResult, GetVideoTimelineRequest, Image, ImageDescriptor, ImageId, ImageSourceId,
    JsonObject, ListVideoSourcesRequest, PackagedMediaAsset, Page, ResolvedMediaAsset,
    UtcTimestamp, VideoClip, VideoClipId, VideoClipStream, VideoProvider, VideoSource,
    VideoTimeline,
};
use reqwest::{Client, Response, Url, header};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::opentrons::OpentronsClient;

const MAX_ASSET_BYTES: usize = 24 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct ArchivedVideos {
    base: Url,
    http: Client,
}

impl ArchivedVideos {
    pub fn new(address: &str) -> Result<Self, A2aLabError> {
        let base = Url::parse(address.trim())
            .map_err(|_| A2aLabError::invalid("video archive URL", "expected an HTTP URL"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(A2aLabError::invalid(
                "video archive URL",
                "expected an HTTP URL without credentials, a query, or a fragment",
            ));
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|_| {
                A2aLabError::unavailable("could not initialize the video archive client")
            })?;
        Ok(Self { base, http })
    }

    async fn get(&self, path: &[&str], query: &[(&str, String)]) -> Result<Response, A2aLabError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| A2aLabError::invalid("video archive URL", "cannot append path"))?
            .pop_if_empty()
            .extend(path);
        let response = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|_| A2aLabError::unavailable("video archive request failed"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(A2aLabError::not_found("recorded video", path.join("/")));
        }
        response.error_for_status().map_err(|error| {
            A2aLabError::unavailable(format!(
                "video archive request failed: {}",
                error.status().unwrap_or_default()
            ))
        })
    }

    async fn json<T: DeserializeOwned>(
        &self,
        path: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, A2aLabError> {
        let data = read_body(self.get(path, query).await?, MAX_METADATA_BYTES).await?;
        serde_json::from_slice(&data)
            .map_err(|_| A2aLabError::unavailable("video archive returned invalid metadata"))
    }

    pub(crate) async fn add_sample_recordings(
        &self,
        robot: &OpentronsClient,
        run_id: &str,
        result: &mut Value,
    ) -> Option<()> {
        let scenario = self.archive_scenario(robot, run_id).await?;
        let commands = recorded_commands(robot, run_id, result).await?;
        annotate_recordings(&scenario, &commands, result)
    }

    async fn archive_scenario(&self, robot: &OpentronsClient, run_id: &str) -> Option<Value> {
        let run = robot
            .call("GET", &format!("runs/{run_id}"), None)
            .await
            .ok()?;
        if run.get("replay").is_some_and(|replay| !replay.is_null()) {
            return None;
        }
        let protocol_id = run["protocolId"].as_str()?;
        let protocol = robot
            .call("GET", &format!("protocols/{protocol_id}"), None)
            .await
            .ok()?;
        let slug = protocol["files"][0]["name"].as_str()?.strip_suffix(".py")?;
        let catalog: Value = self.json(&["scenarios"], &[]).await.ok()?;
        catalog["scenarios"]
            .as_array()?
            .iter()
            .find(|row| row["slug"] == slug)
            .cloned()
    }
}

async fn recorded_commands(robot: &OpentronsClient, run_id: &str, result: &Value) -> Option<Value> {
    if result.is_array() {
        return Some(result.clone());
    }
    robot
        .call(
            "GET",
            &format!("runs/{run_id}/commandsAsPreSerializedList"),
            None,
        )
        .await
        .ok()
}

enum RecordingAnnotation {
    Stop,
    Skip,
    Matched(Value),
    Replace(String),
}

fn annotate_recordings(scenario: &Value, commands: &Value, result: &mut Value) -> Option<()> {
    let single = !result.is_array();
    let result_id = result["id"].clone();
    let reference_commands = scenario["commands"].as_array()?;
    let mut annotated = Vec::new();
    for (index, (record, reference)) in commands
        .as_array()?
        .iter()
        .zip(reference_commands)
        .enumerate()
    {
        match command_annotation(scenario, record, reference, single, &result_id)? {
            RecordingAnnotation::Stop => break,
            RecordingAnnotation::Skip => {}
            RecordingAnnotation::Matched(recording) => {
                result["recording"] = recording;
                return Some(());
            }
            RecordingAnnotation::Replace(command) => annotated.push((index, command)),
        }
    }
    if let Some(records) = result.as_array_mut() {
        for (index, replacement) in annotated {
            records[index] = Value::String(replacement);
        }
    }
    Some(())
}

fn command_annotation(
    scenario: &Value,
    record: &Value,
    reference: &Value,
    single: bool,
    result_id: &Value,
) -> Option<RecordingAnnotation> {
    let mut command: Value = serde_json::from_str(record.as_str()?).ok()?;
    if command["commandType"] != reference["commandType"] {
        return Some(RecordingAnnotation::Stop);
    }
    let at = timestamp(reference["completedAt"].as_str()?)?;
    let angles = covered_angles(scenario, at);
    if angles.is_empty() {
        return Some(RecordingAnnotation::Skip);
    }
    let recording = recording_value(scenario, reference, &angles);
    if single && command["id"] == *result_id {
        return Some(RecordingAnnotation::Matched(recording));
    }
    if !single {
        command["recording"] = recording;
        return Some(RecordingAnnotation::Replace(command.to_string()));
    }
    Some(RecordingAnnotation::Skip)
}

fn covered_angles(scenario: &Value, at: i64) -> Vec<&str> {
    ["top", "side"]
        .into_iter()
        .filter(|angle| camera_covers(&scenario["cameras"][*angle], at))
        .collect()
}

fn camera_covers(coverage: &Value, at: i64) -> bool {
    let start = coverage["start"].as_str().and_then(timestamp);
    let end = coverage["end"].as_str().and_then(timestamp);
    start
        .zip(end)
        .is_some_and(|(start, end)| start <= at && at < end)
}

fn recording_value(scenario: &Value, reference: &Value, angles: &[&str]) -> Value {
    json!({
        "runId": scenario["run_id"],
        "commandId": reference["id"],
        "startedAt": reference["startedAt"],
        "completedAt": reference["completedAt"],
        "sourceIds": angles,
    })
}

fn timestamp(value: &str) -> Option<i64> {
    UtcTimestamp::parse(value).ok().map(UtcTimestamp::as_micros)
}

impl VideoProvider for ArchivedVideos {
    async fn list_video_sources(
        &self,
        request: ListVideoSourcesRequest,
    ) -> Result<Page<VideoSource>, A2aLabError> {
        let sources: Vec<VideoSource> = self.json(&["sources"], &[]).await?;
        let start: usize = request
            .page()
            .cursor()
            .unwrap_or("0")
            .parse()
            .map_err(|_| A2aLabError::invalid("cursor", "invalid page cursor"))?;
        if start > sources.len() {
            return Err(A2aLabError::invalid("cursor", "is past the end"));
        }
        let end = start
            .saturating_add(request.page().limit() as usize)
            .min(sources.len());
        let next = (end < sources.len()).then(|| end.to_string());
        Ok(Page::new(
            sources.into_iter().skip(start).take(end - start).collect(),
            next,
        ))
    }

    async fn get_video_timeline(
        &self,
        request: GetVideoTimelineRequest,
    ) -> Result<VideoTimeline, A2aLabError> {
        self.json(&["timeline", request.source_id().as_str()], &[])
            .await
    }

    async fn get_video_frame_at(
        &self,
        request: GetVideoFrameAtRequest,
    ) -> Result<GetVideoFrameAtResult, A2aLabError> {
        let source = request.source_id().as_str();
        let at = request.at();
        let response = self
            .get(&["frame", source], &[("at", at.to_rfc3339())])
            .await?;
        let captured_at = UtcTimestamp::parse(&read_header::<String>(&response, "x-captured-at")?)?;
        if let Some(limit) = request.max_delta_us()
            && captured_at.as_micros().abs_diff(at.as_micros()) > limit
        {
            return Err(A2aLabError::not_found("recorded frame", at.to_rfc3339()));
        }
        let width = read_header(&response, "x-frame-width")?;
        let height = read_header(&response, "x-frame-height")?;
        let descriptor = ImageDescriptor::new(
            ImageId::new(format!("{source}-{}", captured_at.as_micros()))?,
            ImageSourceId::new(source)?,
            captured_at,
            "image/jpeg",
            width,
            height,
            Some("Recorded replay".to_owned()),
            JsonObject::empty(),
        )?;
        let jpeg = read_body(
            response,
            usize::try_from(DEFAULT_MAX_IMAGE_BYTES).unwrap_or(usize::MAX),
        )
        .await?;
        Ok(GetVideoFrameAtResult::new(
            at,
            Image::new(descriptor, jpeg)?,
        ))
    }

    async fn get_video_clip(&self, request: GetVideoClipRequest) -> Result<VideoClip, A2aLabError> {
        let range = request.range();
        self.json(
            &["clip", request.source_id().as_str()],
            &[
                ("start", range.start().to_rfc3339()),
                ("end", range.end().to_rfc3339()),
            ],
        )
        .await
    }

    fn open_video_clip(
        &self,
        id: &VideoClipId,
    ) -> impl Future<Output = Result<VideoClipStream, A2aLabError>> {
        std::future::ready(Err(A2aLabError::not_found(
            "JPEG clip stream",
            id.to_string(),
        )))
    }

    async fn resolve_clip_playback_asset(
        &self,
        id: &VideoClipId,
        relative_path: Option<&str>,
    ) -> Result<ResolvedMediaAsset, A2aLabError> {
        let filename = relative_path.unwrap_or("index.m3u8");
        if filename.contains("..")
            || !filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            return Err(A2aLabError::invalid(
                "asset",
                "expected a single media filename",
            ));
        }
        let response = self.get(&["media", id.as_str(), filename], &[]).await?;
        let content_type: String = read_header(&response, header::CONTENT_TYPE.as_str())?;
        let data = read_body(response, MAX_ASSET_BYTES).await?;
        Ok(ResolvedMediaAsset::Bytes(PackagedMediaAsset::new(
            content_type,
            data,
        )?))
    }
}

fn read_header<T: std::str::FromStr>(
    response: &Response,
    name: &'static str,
) -> Result<T, A2aLabError> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| A2aLabError::unavailable(format!("video archive returned invalid {name}")))
}

async fn read_body(mut response: Response, limit: usize) -> Result<Vec<u8>, A2aLabError> {
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| A2aLabError::unavailable("could not read video archive response"))?
    {
        if chunk.len() > limit.saturating_sub(data.len()) {
            return Err(A2aLabError::unavailable(
                "video archive response is too large",
            ));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn micros(value: &str) -> i64 {
        timestamp(value).expect("rfc3339")
    }

    fn reference(id: &str, command_type: &str, started_at: &str, completed_at: &str) -> Value {
        json!({
            "id": id,
            "commandType": command_type,
            "startedAt": started_at,
            "completedAt": completed_at,
        })
    }

    fn scenario(commands: &Value) -> Value {
        json!({
            "run_id": "scenario-run",
            "commands": commands,
            "cameras": {
                "top": {
                    "start": "2024-01-01T00:00:00Z",
                    "end": "2024-01-01T00:00:02Z",
                },
                "side": {
                    "start": "2024-01-01T00:00:05Z",
                    "end": "2024-01-01T00:00:08Z",
                },
            },
        })
    }

    #[test]
    fn camera_coverage_is_half_open() {
        let coverage = json!({
            "start": "2024-01-01T00:00:00Z",
            "end": "2024-01-01T00:00:02Z",
        });
        assert!(camera_covers(&coverage, micros("2024-01-01T00:00:00Z")));
        assert!(camera_covers(&coverage, micros("2024-01-01T00:00:01Z")));
        assert!(!camera_covers(&coverage, micros("2024-01-01T00:00:02Z")));
        assert!(!camera_covers(&coverage, micros("2023-12-31T23:59:59Z")));
        assert!(!camera_covers(
            &json!({"start": "2024-01-01T00:00:00Z"}),
            micros("2024-01-01T00:00:01Z")
        ));
    }

    #[test]
    fn single_command_annotation_matches_id_only_inside_coverage() {
        let active = scenario(&json!([reference(
            "cmd-1",
            "aspirate",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:01Z",
        )]));
        let commands = json!([r#"{"id":"cmd-1","commandType":"aspirate"}"#]);
        let mut matched = json!({"id":"cmd-1","commandType":"aspirate"});
        assert_eq!(
            annotate_recordings(&active, &commands, &mut matched),
            Some(())
        );
        assert_eq!(matched["recording"]["runId"], json!("scenario-run"));
        assert_eq!(matched["recording"]["commandId"], json!("cmd-1"));
        assert_eq!(
            matched["recording"]["startedAt"],
            json!("2024-01-01T00:00:00Z")
        );
        assert_eq!(
            matched["recording"]["completedAt"],
            json!("2024-01-01T00:00:01Z")
        );
        assert_eq!(matched["recording"]["sourceIds"], json!(["top"]));

        let mut unmatched = json!({"id":"other","commandType":"aspirate"});
        assert_eq!(
            annotate_recordings(&active, &commands, &mut unmatched),
            Some(())
        );
        assert!(unmatched.get("recording").is_none());

        let inactive = scenario(&json!([reference(
            "cmd-1",
            "aspirate",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:02Z",
        )]));
        let mut boundary = json!({"id":"cmd-1","commandType":"aspirate"});
        assert_eq!(
            annotate_recordings(&inactive, &commands, &mut boundary),
            Some(())
        );
        assert!(boundary.get("recording").is_none());
    }

    #[test]
    fn command_list_replaces_covered_rows_and_stops_on_type_mismatch() {
        let catalog = scenario(&json!([
            reference(
                "cmd-1",
                "aspirate",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:01Z",
            ),
            reference(
                "cmd-2",
                "aspirate",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:02Z",
            ),
            reference(
                "cmd-3",
                "dispense",
                "2024-01-01T00:00:05Z",
                "2024-01-01T00:00:06Z",
            ),
            reference(
                "cmd-4",
                "blowout",
                "2024-01-01T00:00:05Z",
                "2024-01-01T00:00:06Z",
            ),
        ]));
        let skipped = r#"{"id":"cmd-skip","commandType":"aspirate"}"#;
        let mismatch = r#"{"id":"cmd-3","commandType":"aspirate"}"#;
        let later = r#"{"id":"cmd-4","commandType":"blowout"}"#;
        let mut records = json!([
            r#"{"id":"cmd-1","commandType":"aspirate"}"#,
            skipped,
            mismatch,
            later,
        ]);
        let commands = records.clone();
        assert_eq!(
            annotate_recordings(&catalog, &commands, &mut records),
            Some(())
        );
        let rows = records.as_array().expect("list");
        let first: Value = serde_json::from_str(rows[0].as_str().expect("json")).expect("command");
        assert_eq!(first["recording"]["sourceIds"], json!(["top"]));
        assert_eq!(first["recording"]["commandId"], json!("cmd-1"));
        assert_eq!(rows[1], json!(skipped));
        assert_eq!(rows[2], json!(mismatch));
        assert_eq!(rows[3], json!(later));
    }

    #[test]
    fn invalid_command_json_leaves_earlier_rows_unchanged() {
        let catalog = scenario(&json!([
            reference(
                "cmd-1",
                "aspirate",
                "2024-01-01T00:00:00Z",
                "2024-01-01T00:00:01Z",
            ),
            reference(
                "cmd-2",
                "aspirate",
                "2024-01-01T00:00:02Z",
                "2024-01-01T00:00:03Z",
            ),
        ]));
        let first = r#"{"id":"cmd-1","commandType":"aspirate"}"#;
        let mut records = json!([first, "not-json"]);
        let commands = records.clone();
        assert_eq!(annotate_recordings(&catalog, &commands, &mut records), None);
        assert_eq!(records[0], json!(first));
    }
}
