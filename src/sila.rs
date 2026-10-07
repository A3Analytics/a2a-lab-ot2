//! `SiLA` listener settings for the OT-2 adapter.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use a2a_lab_dev_kit::sila::{SilaCertificate, SilaIdentity, SilaServer};
use a2a_lab_dev_kit::{A2aLabApi, A2aLabError};

/// Stable development UUID for the OT-2 `SiLA` server.
pub const DEFAULT_SILA_UUID: &str = "0e2a0002-0000-4000-8000-000000000002";

/// Default `SiLA` gRPC port.
pub const DEFAULT_SILA_PORT: u16 = 50052;

/// Operator settings for the OT-2 `SiLA` listener.
#[derive(Debug, Clone)]
pub struct SilaConfig {
    /// Lowercase server `UUID`.
    pub uuid: String,
    /// Bind host.
    pub host: String,
    /// Bind port. `0` selects an ephemeral port.
    pub port: u16,
    /// File that stores a renamed server name.
    pub name_path: Option<PathBuf>,
    /// File that stores server-initiated `SiLA` clients.
    pub connection_store: Option<PathBuf>,
    /// Advertise `_sila._tcp.local.` after the listener is ready.
    pub announce: bool,
    /// Operator certificate `PEM`. Set together with the key and CA.
    pub cert_pem: Option<String>,
    /// Operator private key `PEM`.
    pub key_pem: Option<String>,
    /// Operator CA `PEM`.
    pub ca_pem: Option<String>,
}

impl Default for SilaConfig {
    fn default() -> Self {
        Self {
            uuid: DEFAULT_SILA_UUID.to_owned(),
            host: "127.0.0.1".to_owned(),
            port: DEFAULT_SILA_PORT,
            name_path: None,
            connection_store: None,
            announce: false,
            cert_pem: None,
            key_pem: None,
            ca_pem: None,
        }
    }
}

/// Identity, certificate, and socket selected for one listener.
#[derive(Debug, Clone)]
pub struct PreparedSila {
    /// Server identity.
    pub identity: SilaIdentity,
    /// TLS material. A self-signed development certificate when no `PEM` was supplied.
    pub certificate: SilaCertificate,
    /// Address passed to [`SilaServer::serve`].
    pub address: SocketAddr,
}

/// Builds the OT-2 identity and either the supplied `PEM` material or a self-signed certificate.
pub fn prepare_sila(config: &SilaConfig) -> Result<PreparedSila, A2aLabError> {
    let mut identity = SilaIdentity::new(
        &config.uuid,
        "opentrons-ot2",
        "OpentronsOt2",
        "SiLA 2 Feature Provider for the OT-2 robot-server adapter.",
        env!("CARGO_PKG_VERSION"),
        "https://github.com/A3Analytics/a2a-lab-ot2",
    )?;
    if let Some(path) = &config.name_path {
        identity = identity.persist_name(path.clone())?;
    }
    let certificate = match (&config.cert_pem, &config.key_pem, &config.ca_pem) {
        (Some(cert), Some(key), Some(ca)) => {
            SilaCertificate::from_pem(cert, key, ca, &config.uuid)?
        }
        (None, None, None) => SilaCertificate::self_signed(&config.uuid)?,
        _ => {
            return Err(A2aLabError::invalid(
                "certificate",
                "certificate, key, and CA PEM are required together",
            ));
        }
    };
    let address = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|_| {
            A2aLabError::invalid("sila_address", "host and port are not a socket address")
        })?;
    Ok(PreparedSila {
        identity,
        certificate,
        address,
    })
}

/// Serves `lab` with the prepared identity, certificate, connection store, and discovery flag.
#[must_use]
pub fn sila_server(
    prepared: PreparedSila,
    lab: Arc<dyn A2aLabApi>,
    config: &SilaConfig,
) -> SilaServer {
    let mut server = SilaServer::new(prepared.identity, lab).certificate(prepared.certificate);
    if let Some(path) = &config.connection_store {
        server = server.connection_store(path.clone());
    }
    if config.announce {
        server = server.announce();
    }
    server
}
