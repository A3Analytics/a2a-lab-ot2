mod support;

use std::path::PathBuf;

use a2a_lab_dev_kit::{
    GetTaskStatusRequest, JsonObject, ListLogSourcesRequest, ListMetricsRequest, ListTasksRequest,
    LogLevel, LogProvider, MetricProvider, PageRequest, QueryLogsRequest, QueryMetricRequest,
    RunId, SourceId, StartTaskRequest, TaskId, TaskProvider, TaskState, TimeRange, UtcTimestamp,
};
use a2a_lab_ot2::OpentronsLab;
use support::Mock;

fn page() -> PageRequest {
    PageRequest::new(None, 200).unwrap()
}

fn range() -> TimeRange {
    TimeRange::new(
        UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
        UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
    )
    .unwrap()
}

fn protocol() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocols/serial_dilution.py")
}

async fn lab() -> (OpentronsLab, Mock) {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    (OpentronsLab::new(&base, protocol()).unwrap(), mock)
}

async fn readonly_lab() -> (OpentronsLab, Mock) {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    (
        OpentronsLab::new(&base, protocol())
            .unwrap()
            .with_readonly(true),
        mock,
    )
}

#[tokio::test]
async fn lists_tasks_sources_and_metrics() {
    let (lab, _) = lab().await;
    let tasks = lab
        .list_tasks(ListTasksRequest { page: page() })
        .await
        .unwrap();
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "run_serial_dilution")
    );
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "execute_command")
    );
    let sources = lab
        .list_sources(ListLogSourcesRequest { page: page() })
        .await
        .unwrap();
    let ids: Vec<_> = sources
        .items()
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert!(ids.contains(&"run_commands"));
    assert!(ids.contains(&"run_command_errors"));
    assert!(ids.contains(&"api.log"));
    assert!(ids.contains(&"serial.log"));
    assert!(ids.contains(&"combined_api_server.log"));
    let metrics = lab
        .list_metrics(ListMetricsRequest { page: page() })
        .await
        .unwrap();
    assert!(
        metrics
            .items()
            .iter()
            .any(|item| item.id.as_str() == "healthy")
    );
    assert!(
        metrics
            .items()
            .iter()
            .any(|item| item.id.as_str() == "door_open")
    );
    assert!(
        metrics
            .items()
            .iter()
            .any(|item| item.id.as_str() == "instrument_count")
    );
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "get_protocols")
    );
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "get_modules")
    );
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "get_system_time")
    );
    assert!(ids.contains(&"kernel.log"));
    assert!(ids.contains(&"protocol_analyses"));
}

#[tokio::test]
async fn advertises_task_schemas() {
    let (lab, _) = lab().await;
    let tasks = lab
        .list_tasks(ListTasksRequest { page: page() })
        .await
        .unwrap();
    let dilution = tasks
        .items()
        .iter()
        .find(|item| item.id.as_str() == "run_serial_dilution")
        .unwrap();
    assert!(
        dilution
            .input_schema
            .as_deref()
            .is_some_and(|schema| schema.contains("object"))
    );
    assert!(dilution.output_schema.is_some());
    let pause = tasks
        .items()
        .iter()
        .find(|item| item.id.as_str() == "pause_run")
        .unwrap();
    assert!(
        pause
            .input_schema
            .as_deref()
            .is_some_and(|schema| schema.contains("run_id"))
    );
    let get_run = tasks
        .items()
        .iter()
        .find(|item| item.id.as_str() == "get_run")
        .unwrap();
    assert!(
        get_run
            .input_schema
            .as_deref()
            .is_some_and(|schema| schema.contains("runId"))
    );
}

#[tokio::test]
async fn runs_serial_dilution_and_maps_status() {
    let (lab, mock) = lab().await;
    let started = lab
        .start(StartTaskRequest::new(
            TaskId::new("run_serial_dilution").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(started.state, TaskState::Working);
    assert_eq!(started.message.as_deref(), Some("running"));
    assert_eq!(started.progress, Some(1.0));
    assert!(started.result.is_some());
    assert!(started.error_kind.is_none());
    let actions = mock.actions().await;
    assert!(actions.iter().any(|(_, action)| action == "play"));
    let status = lab
        .status(GetTaskStatusRequest {
            id: started.id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(status.state, TaskState::Working);
}

#[tokio::test]
async fn pauses_resumes_stops_and_deletes() {
    let (lab, mock) = lab().await;
    let started = lab
        .start(StartTaskRequest::new(
            TaskId::new("run_serial_dilution").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    let run_id = started.id.as_str();
    let paused = lab
        .start(StartTaskRequest::new(
            TaskId::new("pause_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(paused.state, TaskState::Completed);
    let resumed = lab
        .start(StartTaskRequest::new(
            TaskId::new("resume_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);
    let stopped = lab
        .start(StartTaskRequest::new(
            TaskId::new("stop_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(stopped.state, TaskState::Completed);
    lab.start(StartTaskRequest::new(
        TaskId::new("delete_run").unwrap(),
        JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
    ))
    .await
    .unwrap();
    let actions = mock.actions().await;
    let types: Vec<_> = actions.iter().map(|(_, action)| action.as_str()).collect();
    assert!(types.contains(&"pause"));
    assert!(types.contains(&"play"));
    assert!(types.contains(&"stop"));
}

#[tokio::test]
async fn recovery_and_stateless_commands() {
    let (lab, _) = lab().await;
    let started = lab
        .start(StartTaskRequest::new(
            TaskId::new("run_serial_dilution").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    let recovered = lab
        .start(StartTaskRequest::new(
            TaskId::new("resume_from_recovery").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{}"}}"#, started.id)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(recovered.state, TaskState::Completed);
    let command = lab
        .start(StartTaskRequest::new(
            TaskId::new("execute_command").unwrap(),
            JsonObject::parse(r#"{"commandType":"home","params":{}}"#).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(command.state, TaskState::Completed);
    assert!(command.id.as_str().starts_with("cmd-"));
    assert!(command.result.is_some());
    let failed = lab
        .start(StartTaskRequest::new(
            TaskId::new("execute_command").unwrap(),
            JsonObject::parse(r#"{"commandType":"fail","params":{}}"#).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.error_kind.as_deref(), Some("RoboticsControlError"));
    assert_eq!(failed.error_identifier.as_deref(), Some("err-fail"));
}

#[tokio::test]
async fn rejects_bad_input_and_unknown_ids() {
    let (lab, _) = lab().await;
    let missing = lab
        .start(StartTaskRequest::new(
            TaskId::new("missing").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
    let bad_pause = lab
        .start(StartTaskRequest::new(
            TaskId::new("pause_run").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap_err();
    assert_eq!(bad_pause.code(), "invalid");
    let bad_command = lab
        .start(StartTaskRequest::new(
            TaskId::new("execute_command").unwrap(),
            JsonObject::parse(r#"{"params":[]}"#).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(bad_command.code(), "invalid");
    let missing_run = lab
        .status(GetTaskStatusRequest {
            id: RunId::new("run-999").unwrap(),
        })
        .await
        .unwrap_err();
    assert_eq!(missing_run.code(), "not_found");
    let missing_source = LogProvider::query(
        &lab,
        QueryLogsRequest {
            source_id: SourceId::new("nope").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(missing_source.code(), "not_found");
}

#[tokio::test]
async fn queries_logs_and_metrics() {
    let (lab, _) = lab().await;
    lab.start(StartTaskRequest::new(
        TaskId::new("run_serial_dilution").unwrap(),
        JsonObject::empty(),
    ))
    .await
    .unwrap();
    let logs = LogProvider::query(
        &lab,
        QueryLogsRequest {
            source_id: SourceId::new("run_commands").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(!logs.items().is_empty());
    assert!(logs.items().iter().any(|record| {
        record.level == LogLevel::Error
            && (record.message.contains("TipNotAttachedError")
                || record.message.contains("NoTipAttachedError"))
    }));
    assert!(
        logs.items()
            .iter()
            .all(|record| !record.message.contains("errorCode"))
    );
    assert!(
        logs.items()
            .iter()
            .any(|record| record.level == LogLevel::Info)
    );
    let journal = LogProvider::query(
        &lab,
        QueryLogsRequest {
            source_id: SourceId::new("api.log").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(!journal.items().is_empty());
    assert!(
        journal
            .items()
            .iter()
            .any(|record| record.message.contains("Virtual Smoothie")
                || record.message.contains("ENABLE_VIRTUAL_SMOOTHIE")
                || record.message.contains("protocol_engine"))
    );
    assert!(
        journal
            .items()
            .iter()
            .any(|record| record.level == LogLevel::Error)
    );
    let health = MetricProvider::query(
        &lab,
        QueryMetricRequest {
            metric_id: a2a_lab_dev_kit::MetricId::new("healthy").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!((health.items()[0].value - 1.0).abs() < f64::EPSILON);
    let past = TimeRange::new(
        UtcTimestamp::parse("1990-01-01T00:00:00Z").unwrap(),
        UtcTimestamp::parse("1990-01-02T00:00:00Z").unwrap(),
    )
    .unwrap();
    let empty = MetricProvider::query(
        &lab,
        QueryMetricRequest {
            metric_id: a2a_lab_dev_kit::MetricId::new("healthy").unwrap(),
            range: past,
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(empty.items().is_empty());
}

#[tokio::test]
async fn queries_gauges_and_extra_logs() {
    let (lab, _) = lab().await;
    lab.start(StartTaskRequest::new(
        TaskId::new("run_serial_dilution").unwrap(),
        JsonObject::empty(),
    ))
    .await
    .unwrap();
    let door = MetricProvider::query(
        &lab,
        QueryMetricRequest {
            metric_id: a2a_lab_dev_kit::MetricId::new("door_open").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!((door.items()[0].value - 0.0).abs() < f64::EPSILON);
    let errors = LogProvider::query(
        &lab,
        QueryLogsRequest {
            source_id: SourceId::new("run_command_errors").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(
        errors
            .items()
            .iter()
            .all(|record| record.level == LogLevel::Error)
    );
    let kernel = LogProvider::query(
        &lab,
        QueryLogsRequest {
            source_id: SourceId::new("kernel.log").unwrap(),
            range: range(),
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(kernel.items().is_empty());
}

#[tokio::test]
async fn paginates_task_list() {
    let (lab, _) = lab().await;
    let first = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 2).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(first.items().len(), 2);
    assert!(first.next_cursor().is_some());
    let second = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(first.next_cursor().map(str::to_owned), 20).unwrap(),
        })
        .await
        .unwrap();
    assert!(!second.items().is_empty());
}

#[tokio::test]
async fn primitive_http_tasks_complete() {
    let (lab, _) = lab().await;
    lab.start(StartTaskRequest::new(
        TaskId::new("run_serial_dilution").unwrap(),
        JsonObject::empty(),
    ))
    .await
    .unwrap();
    let protocols = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_protocols").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(protocols.state, TaskState::Completed);
    assert!(
        protocols
            .message
            .as_ref()
            .is_some_and(|text| text.contains("protocol"))
    );
    assert!(
        protocols
            .result
            .as_ref()
            .is_some_and(|result| result.to_string().contains("protocol"))
    );
    let door = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_door_status").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(door.state, TaskState::Completed);
    let modules = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_modules").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(modules.state, TaskState::Completed);
    let settings = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_settings").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(settings.state, TaskState::Completed);
}

#[tokio::test]
async fn inventory_is_advertised() {
    use a2a_lab_ot2::opentrons::{COMPOSITE_TASK_IDS, ENTRIES, Kind};

    let (lab, _) = lab().await;
    let tasks = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    let task_ids: Vec<_> = tasks
        .items()
        .iter()
        .map(|item| item.id.as_str().to_owned())
        .collect();
    for id in COMPOSITE_TASK_IDS {
        assert!(task_ids.iter().any(|item| item == id), "{id}");
    }
    for entry in ENTRIES {
        match entry.kind {
            Kind::Task => assert!(task_ids.iter().any(|item| item == entry.id), "{}", entry.id),
            Kind::Skip => assert!(
                !task_ids.iter().any(|item| item == entry.id),
                "{}",
                entry.id
            ),
            Kind::Log | Kind::Metric => {}
        }
    }
    let sources = lab
        .list_sources(ListLogSourcesRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    let source_ids: Vec<_> = sources
        .items()
        .iter()
        .map(|item| item.id.as_str().to_owned())
        .collect();
    let metrics = lab
        .list_metrics(ListMetricsRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    let metric_ids: Vec<_> = metrics
        .items()
        .iter()
        .map(|item| item.id.as_str().to_owned())
        .collect();
    for entry in ENTRIES {
        match entry.kind {
            Kind::Log => assert!(
                source_ids.iter().any(|item| item == entry.id),
                "{}",
                entry.id
            ),
            Kind::Metric => {
                assert!(
                    metric_ids.iter().any(|item| item == entry.id),
                    "{}",
                    entry.id
                );
            }
            Kind::Task | Kind::Skip => {}
        }
    }
}

#[tokio::test]
async fn readonly_lists_get_tasks_and_omits_writes() {
    use a2a_lab_ot2::opentrons::{COMPOSITE_TASK_IDS, ENTRIES, Kind, is_read};

    let (lab, _) = readonly_lab().await;
    let tasks = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    let task_ids: Vec<_> = tasks
        .items()
        .iter()
        .map(|item| item.id.as_str().to_owned())
        .collect();
    for id in COMPOSITE_TASK_IDS {
        assert!(!task_ids.iter().any(|item| item == id), "{id}");
    }
    for entry in ENTRIES {
        if is_read(entry) {
            assert!(task_ids.iter().any(|item| item == entry.id), "{}", entry.id);
        } else if entry.kind == Kind::Task {
            assert!(
                !task_ids.iter().any(|item| item == entry.id),
                "{}",
                entry.id
            );
        }
    }
    let sources = lab
        .list_sources(ListLogSourcesRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        sources
            .items()
            .iter()
            .any(|item| item.id.as_str() == "api.log")
    );
    let metrics = lab
        .list_metrics(ListMetricsRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        metrics
            .items()
            .iter()
            .any(|item| item.id.as_str() == "healthy")
    );
}

#[tokio::test]
async fn readonly_starts_get_tasks_and_rejects_writes() {
    let (lab, mock) = readonly_lab().await;
    let protocols = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_protocols").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(protocols.state, TaskState::Completed);
    let door = lab
        .start(StartTaskRequest::new(
            TaskId::new("get_door_status").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(door.state, TaskState::Completed);
    for id in [
        "run_serial_dilution",
        "pause_run",
        "execute_command",
        "post_runs",
    ] {
        let error = lab
            .start(StartTaskRequest::new(
                TaskId::new(id).unwrap(),
                JsonObject::empty(),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "not_found", "{id}");
    }
    assert!(mock.actions().await.is_empty());
}
