use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabResult, ComplianceCheck, ComplianceOutcome,
    ComplianceRunnerConfig, ComplianceSuite, FixtureCapability, GetImageRequest,
    ImplementationIdentity, ListTasksRequest, McpLab, PageRequest, StartTaskRequest,
    compliance_profile, run_compliance,
};
use a2a_lab_ot2::{
    FixtureConfig, FixtureReadiness, FixtureServer, FixtureVariant, fixture_declaration,
};

#[tokio::test]
async fn fixture_serves_real_a2a_and_mcp_with_stable_state() {
    let first = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    assert_eq!(first.readiness().status, "ready");
    first.readiness().fixtures.validate().unwrap();

    let a2a = A2aClient::new(&first.readiness().a2a_url).unwrap();
    let card = a2a.agent_card().await.unwrap();
    assert!(card.skills.iter().any(|skill| skill.id == "get-image"));
    assert!(card.skills.iter().any(|skill| skill.id == "agent-message"));
    let mcp = McpLab::connect(&first.readiness().mcp_url).await.unwrap();
    let listed = mcp
        .execute(A2aLabCommand::ListTasks(ListTasksRequest {
            page: PageRequest::new(None, 2).unwrap(),
        }))
        .await
        .unwrap();
    assert!(matches!(listed.task.result, A2aLabResult::ListTasks(_)));

    let fixtures = fixture_declaration();
    let FixtureCapability::Required(tasks) = fixtures.tasks else {
        panic!("tasks must be required");
    };
    let run = a2a
        .start_task(StartTaskRequest::new(tasks.task_id, tasks.input))
        .await
        .unwrap();
    assert_eq!(run.state, a2a_lab_dev_kit::TaskState::Completed);
    let FixtureCapability::Required(images) = fixtures.images else {
        panic!("images must be required");
    };
    let image = a2a
        .get_image(GetImageRequest::new(images.image_id))
        .await
        .unwrap();
    assert_eq!(image.data().len(), 17);
    let first_declaration = first.readiness().fixtures.clone();
    let first_conversation_dir = first.conversation_dir().to_owned();
    assert!(first_conversation_dir.exists());
    first.shutdown().await;
    assert!(!first_conversation_dir.exists());

    let second = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    assert_eq!(second.readiness().fixtures, first_declaration);
    second.shutdown().await;
}

#[tokio::test]
async fn fixture_shutdown_and_drop_release_all_configured_ports() {
    let a2a_port = unused_port().await;
    let mcp_port = unused_port().await;
    let server = FixtureServer::start(FixtureConfig {
        a2a_port,
        mcp_port,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let conversation_dir = server.conversation_dir().to_owned();
    server.shutdown().await;
    assert!(!conversation_dir.exists());
    tokio::net::TcpListener::bind(("127.0.0.1", a2a_port))
        .await
        .unwrap();
    tokio::net::TcpListener::bind(("127.0.0.1", mcp_port))
        .await
        .unwrap();

    let dropped_a2a_port = unused_port().await;
    let dropped_mcp_port = unused_port().await;
    let server = FixtureServer::start(FixtureConfig {
        a2a_port: dropped_a2a_port,
        mcp_port: dropped_mcp_port,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let dropped_conversation_dir = server.conversation_dir().to_owned();
    drop(server);
    tokio::task::yield_now().await;
    assert!(!dropped_conversation_dir.exists());
    tokio::net::TcpListener::bind(("127.0.0.1", dropped_a2a_port))
        .await
        .unwrap();
    tokio::net::TcpListener::bind(("127.0.0.1", dropped_mcp_port))
        .await
        .unwrap();
}

#[tokio::test]
async fn occupied_port_has_an_actionable_startup_error() {
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    let Err(error) = FixtureServer::start(FixtureConfig {
        a2a_port: port,
        ..FixtureConfig::default()
    })
    .await
    else {
        panic!("occupied port must fail");
    };
    let message = error.to_string();
    assert!(message.contains("fixture A2A bind"), "{message}");
    assert!(message.contains(&port.to_string()), "{message}");
}

#[tokio::test]
async fn basic_suite_runs_only_the_preserved_operation_matrix() {
    let fixture = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let mut config = compliance_config(fixture.readiness(), true);
    config.suite = ComplianceSuite::Basic;
    let report = run_compliance(config).await.unwrap();

    assert!(report.compliant);
    assert_eq!(report.suite_version, "1.1.0");
    assert_eq!(report.selected_suite, ComplianceSuite::Basic);
    assert_eq!(
        report.enabled_checks,
        [ComplianceCheck::BasicOperations].into_iter().collect()
    );
    assert_eq!(report.cases.len(), compliance_profile().cases.len());
    assert!(
        report
            .cases
            .iter()
            .all(|case| !case.required || case.outcome == ComplianceOutcome::Pass)
    );
    assert!(report.scenarios.is_empty());
    assert_eq!(fixture.model_invocations(), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn full_suite_passes_all_linked_interface_journeys_repeatably() {
    let standard = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let report = run_compliance(compliance_config(standard.readiness(), true))
        .await
        .unwrap();
    assert!(report.compliant);
    assert_eq!(report.selected_suite, ComplianceSuite::Full);
    assert_eq!(report.cases.len(), compliance_profile().cases.len());
    assert!(
        report
            .cases
            .iter()
            .all(|case| { !case.required || matches!(case.outcome, ComplianceOutcome::Pass) })
    );
    for case_id in ["tasks.start.success", "tasks.status.success"] {
        assert_eq!(
            report
                .cases
                .iter()
                .find(|case| case.case_id == case_id)
                .unwrap()
                .outcome,
            ComplianceOutcome::Pass
        );
    }
    assert_eq!(report.scenarios.len(), 17);
    assert!(
        report
            .scenarios
            .iter()
            .all(|scenario| scenario.required && scenario.outcome == ComplianceOutcome::Pass)
    );
    for capability in ["logs", "metrics", "tasks", "images"] {
        for path in ["a2a-a2a", "mcp-mcp", "a2a-mcp", "mcp-a2a"] {
            let scenario_id = format!("{capability}.link.{path}");
            assert_eq!(
                report
                    .scenarios
                    .iter()
                    .find(|scenario| scenario.scenario_id == scenario_id)
                    .unwrap()
                    .outcome,
                ComplianceOutcome::Pass
            );
        }
    }
    assert_eq!(
        report
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == "agent-message.a2a")
            .unwrap()
            .outcome,
        ComplianceOutcome::Pass
    );
    assert_eq!(standard.model_invocations(), 2);
    standard.shutdown().await;

    let repeated = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let repeated_report = run_compliance(compliance_config(repeated.readiness(), true))
        .await
        .unwrap();
    assert!(repeated_report.compliant);
    assert_eq!(repeated_report.scenarios, report.scenarios);
    repeated.shutdown().await;
}

#[tokio::test]
async fn controlled_variants_produce_step_specific_full_suite_diagnostics() {
    let mismatched = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::McpMetricError,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let report = run_compliance(compliance_config(mismatched.readiness(), true))
        .await
        .unwrap();
    assert!(!report.compliant);
    let diagnostic = report
        .cases
        .iter()
        .find(|case| case.case_id == "metrics.query.success")
        .unwrap()
        .diagnostics
        .join(" ");
    assert!(diagnostic.contains("normalized interface difference"));
    assert!(diagnostic.contains("a2a="));
    assert!(diagnostic.contains("mcp="));
    assert!(
        report
            .cases
            .iter()
            .filter(|case| !case.case_id.starts_with("metrics.query."))
            .all(|case| !case.required || case.outcome == ComplianceOutcome::Pass)
    );
    assert!(
        report
            .scenarios
            .iter()
            .filter(|scenario| {
                scenario.scenario_id != "metrics.link.a2a-mcp"
                    && scenario.scenario_id != "metrics.link.mcp-mcp"
            })
            .all(|scenario| scenario.outcome == ComplianceOutcome::Pass)
    );
    mismatched.shutdown().await;

    let empty = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::McpEmptyLogSources,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let report = run_compliance(compliance_config(empty.readiness(), true))
        .await
        .unwrap();
    assert!(!report.compliant);
    for scenario_id in ["logs.link.mcp-mcp", "logs.link.mcp-a2a"] {
        let scenario = report
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == scenario_id)
            .unwrap();
        assert_eq!(scenario.outcome, ComplianceOutcome::Fail);
        assert_eq!(
            scenario.diagnostics,
            ["list_log_sources: discovery returned no log source"]
        );
    }
    assert_eq!(
        report
            .scenarios
            .iter()
            .find(|scenario| scenario.scenario_id == "logs.link.a2a-mcp")
            .unwrap()
            .outcome,
        ComplianceOutcome::Pass
    );
    assert!(
        report
            .scenarios
            .iter()
            .filter(|scenario| {
                scenario.scenario_id != "logs.link.mcp-mcp"
                    && scenario.scenario_id != "logs.link.mcp-a2a"
            })
            .all(|scenario| scenario.outcome == ComplianceOutcome::Pass)
    );
    empty.shutdown().await;
}

#[tokio::test]
async fn controlled_agent_failure_is_reported_and_opt_out_skips_the_model() {
    let ungrounded = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::UngroundedAgentReply,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let report = run_compliance(compliance_config(ungrounded.readiness(), true))
        .await
        .unwrap();
    assert!(!report.compliant);
    let scenario = report
        .scenarios
        .iter()
        .find(|scenario| scenario.scenario_id == "agent-message.a2a")
        .unwrap();
    assert_eq!(scenario.outcome, ComplianceOutcome::Fail);
    assert_eq!(scenario.diagnostics.len(), 1);
    assert!(
        scenario.diagnostics[0]
            .starts_with("a2a_agent_message: reply did not preserve MCP task identifier `"),
        "{:?}",
        scenario.diagnostics
    );
    assert_eq!(ungrounded.model_invocations(), 2);
    assert!(
        report
            .cases
            .iter()
            .all(|case| !case.required || case.outcome == ComplianceOutcome::Pass)
    );
    assert!(
        report
            .scenarios
            .iter()
            .filter(|candidate| candidate.scenario_id != "agent-message.a2a")
            .all(|candidate| candidate.outcome == ComplianceOutcome::Pass)
    );
    ungrounded.shutdown().await;

    let opted_out = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let report = run_compliance(compliance_config(opted_out.readiness(), false))
        .await
        .unwrap();
    assert!(report.compliant);
    assert_eq!(report.scenarios.len(), 16);
    assert_eq!(
        report.enabled_checks,
        [
            ComplianceCheck::BasicOperations,
            ComplianceCheck::LinkedScenarios,
        ]
        .into_iter()
        .collect()
    );
    assert!(
        report
            .scenarios
            .iter()
            .all(|scenario| scenario.scenario_id != "agent-message.a2a")
    );
    assert_eq!(opted_out.model_invocations(), 0);
    opted_out.shutdown().await;
}

#[test]
fn fixture_command_ignores_external_dependency_configuration() {
    let temp_root = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-fixture-process-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .arg("fixture")
        .env("TMPDIR", &temp_root)
        .env("A2ALAB_OPENTRONS_URL", "http://unreachable.invalid")
        .env("A2ALAB_OIDC_ISSUER", "https://unreachable.invalid")
        .env("A2ALAB_MODEL_PROVIDER", "openai")
        .env("A2ALAB_MODEL", "invalid-model")
        .env("A2ALAB_BEDROCK_MODEL", "invalid-bedrock-model")
        .env("OPENAI_API_KEY", "invalid-openai-key")
        .env("ANTHROPIC_API_KEY", "invalid-anthropic-key")
        .env("AWS_ACCESS_KEY_ID", "invalid-aws-key")
        .env("AWS_SECRET_ACCESS_KEY", "invalid-aws-secret")
        .env("AWS_SESSION_TOKEN", "invalid-aws-token")
        .env("AWS_REGION", "invalid-aws-region")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let line = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .next()
        .unwrap()
        .unwrap();
    let readiness: FixtureReadiness = serde_json::from_str(&line).unwrap();
    assert_eq!(readiness.status, "ready");
    assert!(readiness.a2a_url.starts_with("http://127.0.0.1:"));
    assert!(readiness.mcp_url.ends_with("/mcp"));
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(temp_root).unwrap();
}

fn compliance_config(
    readiness: &FixtureReadiness,
    agent_message_check: bool,
) -> ComplianceRunnerConfig {
    ComplianceRunnerConfig {
        a2a_url: readiness.a2a_url.clone(),
        mcp_url: readiness.mcp_url.clone(),
        fixtures: readiness.fixtures.clone(),
        implementation: ImplementationIdentity {
            name: "a2a-lab-ot2".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        case_timeout: std::time::Duration::from_secs(2),
        suite: ComplianceSuite::Full,
        agent_message_check,
    }
}

async fn unused_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}
