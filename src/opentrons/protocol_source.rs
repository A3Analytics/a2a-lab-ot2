//! Read-only access to uploaded Python files in the robot-server protocol store.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use a2a_lab_dev_kit::{A2aLabError, JsonObject, TaskDefinition, TaskId};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;

pub(super) const PROTOCOL_PYTHON_TASK: &str = "get_protocol_python";
const MAX_SOURCE_MIB: usize = 5;
const MAX_SOURCE_BYTES: usize = MAX_SOURCE_MIB * 1024 * 1024;

pub(super) fn task_definition() -> TaskDefinition {
    TaskDefinition {
        id: TaskId::new(PROTOCOL_PYTHON_TASK).expect("task id"),
        name: "Get protocol Python source".to_owned(),
        description: "Read the original uploaded Python file for an OT-2 protocol ID.".to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
        input_schema: Some(
            json!({
                "type": "object",
                "properties": {"protocolId": {"type": "string", "format": "uuid"}},
                "required": ["protocolId"],
            })
            .to_string(),
        ),
        output_schema: Some(
            json!({
                "type": "object",
                "properties": {
                    "protocolId": {"type": "string", "format": "uuid"},
                    "filename": {"type": "string"},
                    "data": {"type": "string", "contentEncoding": "base64", "contentMediaType": "text/x-python"},
                },
                "required": ["protocolId", "filename", "data"],
            })
            .to_string(),
        ),
    }
}

pub(super) fn read(root: &Path, protocol_id: &str) -> Result<JsonObject, A2aLabError> {
    if !is_protocol_id(protocol_id) {
        return Err(A2aLabError::invalid("protocolId", "expected a UUID"));
    }

    let directory = root.join(protocol_id);
    let missing = || A2aLabError::not_found("protocol Python source", protocol_id);
    let map_io = |error: io::Error| match error.kind() {
        io::ErrorKind::NotFound => missing(),
        _ => A2aLabError::unavailable("cannot read the protocol source store"),
    };
    if !fs::symlink_metadata(&directory).map_err(map_io)?.is_dir() {
        return Err(missing());
    }

    let mut python_file = None;
    for entry in fs::read_dir(&directory).map_err(map_io)? {
        let entry = entry.map_err(map_io)?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
            || !entry.file_type().map_err(map_io)?.is_file()
        {
            continue;
        }
        if python_file.replace(path).is_some() {
            return Err(A2aLabError::protocol(
                "protocol source directory contains multiple Python files",
            ));
        }
    }
    let path = python_file.ok_or_else(missing)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| A2aLabError::protocol("invalid protocol source filename"))?;
    let mut data = Vec::new();
    File::open(&path)
        .map_err(map_io)?
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .map_err(map_io)?;
    if data.len() > MAX_SOURCE_BYTES {
        return Err(A2aLabError::unavailable(format!(
            "Python source exceeds {MAX_SOURCE_MIB} MiB"
        )));
    }

    JsonObject::try_from_value(json!({
        "protocolId": protocol_id,
        "filename": filename,
        "data": STANDARD.encode(data),
    }))
}

fn is_protocol_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}
