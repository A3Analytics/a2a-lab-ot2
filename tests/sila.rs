use a2a_lab_dev_kit::sila::{SilaCertificate, certificate_matches_profile};
use a2a_lab_dev_kit::{A2aLabService, MemoryLogs, MemoryMetrics, MemoryTasks};
use a2a_lab_ot2::{DEFAULT_SILA_PORT, DEFAULT_SILA_UUID, SilaConfig, prepare_sila, sila_server};

#[test]
fn default_sila_uses_the_ot2_uuid_and_a_matching_certificate() {
    let prepared = prepare_sila(&SilaConfig::default()).unwrap();
    assert_eq!(prepared.identity.server_uuid, DEFAULT_SILA_UUID);
    assert_eq!(prepared.identity.server_type, "OpentronsOt2");
    assert_eq!(prepared.address.port(), DEFAULT_SILA_PORT);
    assert!(certificate_matches_profile(
        &prepared.certificate.cert_pem,
        DEFAULT_SILA_UUID
    ));
}

#[test]
fn operator_pem_replaces_the_self_signed_certificate() {
    let uuid = "11111111-1111-1111-1111-111111111112";
    let generated = SilaCertificate::self_signed(uuid).unwrap();
    let config = SilaConfig {
        uuid: uuid.to_owned(),
        cert_pem: Some(generated.cert_pem.clone()),
        key_pem: Some(generated.key_pem.clone()),
        ca_pem: Some(generated.ca_pem.clone()),
        ..SilaConfig::default()
    };
    let prepared = prepare_sila(&config).unwrap();
    assert_eq!(prepared.certificate, generated);
    assert_eq!(prepared.identity.server_uuid, uuid);
}

#[test]
fn partial_pem_is_rejected() {
    let config = SilaConfig {
        cert_pem: Some("cert".to_owned()),
        ..SilaConfig::default()
    };
    assert_eq!(prepare_sila(&config).unwrap_err().code(), "invalid");
}

#[tokio::test]
async fn default_sila_listener_binds_with_tls() {
    let config = SilaConfig {
        port: 0,
        name_path: Some(
            std::env::temp_dir().join(format!("a2a-lab-ot2-sila-name-{}", std::process::id())),
        ),
        connection_store: Some(std::env::temp_dir().join(format!(
            "a2a-lab-ot2-sila-connections-{}.json",
            std::process::id()
        ))),
        ..SilaConfig::default()
    };
    let prepared = prepare_sila(&config).unwrap();
    let address = prepared.address;
    let lab =
        A2aLabService::new(MemoryLogs::new(), MemoryMetrics::new(), MemoryTasks::new()).share();
    let handle = sila_server(prepared, lab, &config)
        .serve(address)
        .await
        .unwrap();
    assert_ne!(handle.local_addr().port(), 0);
    drop(handle);
    let _ = std::fs::remove_file(config.name_path.unwrap());
    let _ = std::fs::remove_file(config.connection_store.unwrap());
}
