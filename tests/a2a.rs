mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_sdk::{
    A2aClient, A2aServer, GetTaskStatusRequest, JsonObject, LabApi, LabResult, LabService,
    ListTasksRequest, PageRequest, StartTaskRequest, TaskId, TaskState, bind_local,
};
use a2a_lab_sdk_example::OpentronsLab;
use support::Mock;

fn protocol() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocols/serial_dilution.py")
}

async fn serve(service: Arc<dyn LabApi>) -> String {
    let (listener, address) = bind_local().await.unwrap();
    let server = A2aServer::new(&service);
    tokio::spawn(async move {
        server.listen(listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    format!("http://{address}")
}

#[tokio::test]
async fn a2a_run_pause_resume_and_status() {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    let lab = OpentronsLab::new(&base, protocol()).unwrap();
    let service = LabService::new(lab.clone(), lab.clone(), lab).share();
    let client = A2aClient::new(&serve(Arc::clone(&service)).await).unwrap();

    let tasks = client
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "run_serial_dilution")
    );

    let started = client
        .start_task(
            StartTaskRequest::new(
                TaskId::new("run_serial_dilution").unwrap(),
                JsonObject::empty(),
            )
            .immediate(),
        )
        .await
        .unwrap();
    assert_eq!(started.state, TaskState::Working);
    let LabResult::StartTask(run) = started.result else {
        panic!("start result");
    };
    let run_id = run.id;

    let paused = client
        .start_task(StartTaskRequest::new(
            TaskId::new("pause_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(paused.state, TaskState::Completed);

    let resumed = client
        .start_task(StartTaskRequest::new(
            TaskId::new("resume_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);

    let status = client
        .task_status(GetTaskStatusRequest { id: run_id })
        .await
        .unwrap();
    assert_eq!(status.state, TaskState::Working);

    let logs = client
        .query_logs(a2a_lab_sdk::QueryLogsRequest {
            source_id: a2a_lab_sdk::SourceId::new("run_commands").unwrap(),
            range: a2a_lab_sdk::TimeRange::new(
                a2a_lab_sdk::UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
                a2a_lab_sdk::UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
            page: PageRequest::new(None, 20).unwrap(),
        })
        .await
        .unwrap();
    assert!(!logs.items().is_empty());

    let metrics = client
        .query_metric(a2a_lab_sdk::QueryMetricRequest {
            metric_id: a2a_lab_sdk::MetricId::new("healthy").unwrap(),
            range: a2a_lab_sdk::TimeRange::new(
                a2a_lab_sdk::UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
                a2a_lab_sdk::UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
            page: PageRequest::new(None, 10).unwrap(),
        })
        .await
        .unwrap();
    assert!((metrics.items()[0].value - 1.0).abs() < f64::EPSILON);
}
