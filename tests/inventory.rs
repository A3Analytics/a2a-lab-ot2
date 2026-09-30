use a2a_lab_sdk_example::opentrons::{
    COMPOSITE_TASK_IDS, ENTRIES, Kind, OPENAPI_OPERATIONS, covers, is_read,
};

#[test]
fn every_openapi_operation_is_classified() {
    for (method, path) in OPENAPI_OPERATIONS {
        assert!(
            covers(method, path),
            "unclassified OpenAPI operation: {method} {path}"
        );
    }
}

#[test]
fn inventory_ids_are_unique() {
    let mut ids = ENTRIES.iter().map(|entry| entry.id).collect::<Vec<_>>();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before);
}

#[test]
fn read_tasks_are_get_backed() {
    for entry in ENTRIES {
        if entry.kind == Kind::Task {
            assert_eq!(is_read(entry), entry.method == "GET", "{}", entry.id);
        } else {
            assert!(!is_read(entry), "{}", entry.id);
        }
    }
    for id in COMPOSITE_TASK_IDS {
        assert!(
            ENTRIES
                .iter()
                .find(|entry| entry.id == *id)
                .is_none_or(|entry| !is_read(entry)),
            "{id}"
        );
    }
}
