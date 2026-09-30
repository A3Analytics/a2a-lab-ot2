mod support;

use std::path::PathBuf;

use a2a_lab_sdk::{
    GetTaskStatusRequest, JsonObject, ListLogSourcesRequest, ListMetricsRequest, ListTasksRequest,
    LogLevel, LogProvider, MetricProvider, PageRequest, QueryLogsRequest, QueryMetricRequest,
    RunId, SourceId, StartTaskRequest, TaskId, TaskProvider, TaskState, TimeRange, UtcTimestamp,
};
use a2a_lab_sdk_example::OpentronsLab;
use support::Mock;

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
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
    assert!(ids.contains(&"api.log"));
    assert!(ids.contains(&"serial.log"));
    assert!(!ids.contains(&"command_errors"));
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
}

#[tokio::test]
async fn runs_serial_dilution_and_maps_status() {
    let (lab, mock) = lab().await;
    let started = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("run_serial_dilution").unwrap(),
            input: JsonObject::empty(),
        })
        .await
        .unwrap();
    assert_eq!(started.state, TaskState::Working);
    assert_eq!(started.message.as_deref(), Some("running"));
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
        .start(StartTaskRequest {
            task_id: TaskId::new("run_serial_dilution").unwrap(),
            input: JsonObject::empty(),
        })
        .await
        .unwrap();
    let run_id = started.id.as_str();
    let paused = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("pause_run").unwrap(),
            input: JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(paused.state, TaskState::Completed);
    let resumed = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("resume_run").unwrap(),
            input: JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);
    let stopped = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("stop_run").unwrap(),
            input: JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(stopped.state, TaskState::Completed);
    lab.start(StartTaskRequest {
        task_id: TaskId::new("delete_run").unwrap(),
        input: JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
    })
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
        .start(StartTaskRequest {
            task_id: TaskId::new("run_serial_dilution").unwrap(),
            input: JsonObject::empty(),
        })
        .await
        .unwrap();
    let recovered = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("resume_from_recovery").unwrap(),
            input: JsonObject::parse(&format!(r#"{{"run_id":"{}"}}"#, started.id)).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(recovered.state, TaskState::Completed);
    let command = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("execute_command").unwrap(),
            input: JsonObject::parse(r#"{"commandType":"home","params":{}}"#).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(command.state, TaskState::Completed);
    assert!(command.id.as_str().starts_with("cmd-"));
}

#[tokio::test]
async fn rejects_bad_input_and_unknown_ids() {
    let (lab, _) = lab().await;
    let missing = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("missing").unwrap(),
            input: JsonObject::empty(),
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
    let bad_pause = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("pause_run").unwrap(),
            input: JsonObject::empty(),
        })
        .await
        .unwrap_err();
    assert_eq!(bad_pause.code(), "invalid");
    let bad_command = lab
        .start(StartTaskRequest {
            task_id: TaskId::new("execute_command").unwrap(),
            input: JsonObject::parse(r#"{"params":[]}"#).unwrap(),
        })
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
    lab.start(StartTaskRequest {
        task_id: TaskId::new("run_serial_dilution").unwrap(),
        input: JsonObject::empty(),
    })
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
            metric_id: a2a_lab_sdk::MetricId::new("healthy").unwrap(),
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
            metric_id: a2a_lab_sdk::MetricId::new("healthy").unwrap(),
            range: past,
            page: page(),
        },
    )
    .await
    .unwrap();
    assert!(empty.items().is_empty());
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
