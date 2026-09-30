//! A2A + MCP lab agent backed by a local Opentrons robot-server.

use std::path::PathBuf;

use a2a_lab_sdk::{A2aServer, LabService, McpServer, SdkError};

use a2a_lab_sdk_example::OpentronsLab;

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    let base = std::env::var("OPENTRONS_URL").unwrap_or_else(|_| "http://127.0.0.1:31950".into());
    let protocol = protocol_path();
    let lab = OpentronsLab::new(&base, protocol)?;
    lab.client().health().await?;
    let service = LabService::new(lab.clone(), lab.clone(), lab).share();
    let a2a = A2aServer::new(&service);
    let mcp = McpServer::new(&service);
    tokio::try_join!(a2a.listen(None), mcp.serve_http(None))?;
    Ok(())
}

fn protocol_path() -> PathBuf {
    std::env::var_os("OPENTRONS_PROTOCOL").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocols/serial_dilution.py"),
        PathBuf::from,
    )
}
