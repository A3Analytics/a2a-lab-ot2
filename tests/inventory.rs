use a2a_lab_sdk_example::opentrons::{ENTRIES, OPENAPI_OPERATIONS, covers};

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
