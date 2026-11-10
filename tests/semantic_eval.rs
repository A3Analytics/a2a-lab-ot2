//! Deterministic semantic evaluation manifest and read-only validation.

use std::path::PathBuf;

use a2a_lab_dev_kit::{MemoryVideos, UtcTimestamp, VideoSourceId};
use a2a_lab_ot2::videos::dvr::IndexedVideoDvr;
use a2a_lab_ot2::videos::semantic_eval::{
    DvrAccessRoute, EvaluationManifest, EvaluationSplits, EvidenceAvailability, FrameAnnotation,
    LabeledSample, PredicateLabel, PredicateOutcome, RetainedSourceRecord, RetrievalLabel,
    RetrievalRelevance, TimeBlock, discover_retained_sources, validate_manifest,
};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-{name}-{}",
        std::process::id()
    ))
}

fn jpeg(label: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, label, 0xD9]
}

async fn seed_deterministic_dvr(
    dir: &std::path::Path,
) -> (
    IndexedVideoDvr,
    VideoSourceId,
    a2a_lab_dev_kit::DvrTrackId,
    [UtcTimestamp; 5],
) {
    let source = VideoSourceId::new("external-camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let store = IndexedVideoDvr::open(dir, 86_400).await.unwrap();
    let anchor = a2a_lab_ot2::time::now().unwrap();
    let stamps = [
        anchor,
        anchor.saturating_add_micros(1_000_000),
        anchor.saturating_add_micros(2_000_000),
        anchor.saturating_add_micros(300_000_000),
        anchor.saturating_add_micros(301_000_000),
    ];
    for (index, stamp) in stamps.iter().enumerate() {
        let label = u8::try_from(index).expect("fixture uses at most 255 frames");
        store
            .append_mjpeg_frame(&source, *stamp, 1_000_000, &jpeg(label))
            .await
            .unwrap();
    }
    (store, source, track, stamps)
}

async fn build_deterministic_manifest(
    store: &IndexedVideoDvr,
    source: &VideoSourceId,
    track: &a2a_lab_dev_kit::DvrTrackId,
    stamps: [UtcTimestamp; 5],
) -> EvaluationManifest {
    let discovered = discover_retained_sources(store).await.unwrap();
    assert_eq!(discovered.len(), 1);
    let row = &discovered[0];
    let evidence_first = store
        .frame_payload_evidence(source, track, stamps[0].as_micros())
        .await
        .unwrap();
    let evidence_occluded = store
        .frame_payload_evidence(source, track, stamps[3].as_micros())
        .await
        .unwrap();
    EvaluationManifest {
        schema_version: 1,
        manifest_version: "deterministic-v1".to_owned(),
        inventory_disclaimer: "Synthetic DVR fixture for CI; not Chupacabra model accuracy."
            .to_owned(),
        dvr_access: DvrAccessRoute {
            host: "local-fixture".to_owned(),
            read_only: true,
            dvr_root_env: "A2ALAB_VIDEO_DVR_ROOT".to_owned(),
            dvr_root_default: ".a2a-lab-ot2/video-dvr".to_owned(),
            service_unit: None,
        },
        retained_sources: vec![RetainedSourceRecord {
            source_id: row.source_id.clone(),
            track_id: row.track_id.clone(),
            codec: row.codec.clone(),
            capture_start_utc: row.capture_start_utc.clone(),
            capture_end_utc: row.capture_end_utc.clone(),
            frame_count: row.frame_count,
            evidence_availability: EvidenceAvailability::Available,
            aggregate_payload_sha256: None,
        }],
        labeled_sample: LabeledSample {
            frame_annotations: vec![
                FrameAnnotation {
                    annotation_id: "tune-retrieval-a".to_owned(),
                    source_id: source.as_str().to_owned(),
                    track_id: track.as_str().to_owned(),
                    capture_utc: stamps[0].to_string(),
                    start_us: stamps[0].as_micros(),
                    payload_sha256: evidence_first.payload_sha256.clone(),
                    roi: None,
                    retrieval: Some(RetrievalLabel {
                        query_text: "pipette above deck".to_owned(),
                        relevance: RetrievalRelevance::High,
                    }),
                    predicates: vec![PredicateLabel {
                        predicate: "lab.foreign_object_present.v1".to_owned(),
                        outcome: PredicateOutcome::Absent,
                    }],
                    occlusion: None,
                },
                FrameAnnotation {
                    annotation_id: "eval-unknown-b".to_owned(),
                    source_id: source.as_str().to_owned(),
                    track_id: track.as_str().to_owned(),
                    capture_utc: stamps[3].to_string(),
                    start_us: stamps[3].as_micros(),
                    payload_sha256: evidence_occluded.payload_sha256,
                    roi: None,
                    retrieval: None,
                    predicates: vec![PredicateLabel {
                        predicate: "lab.pipette_tip_attached.v1".to_owned(),
                        outcome: PredicateOutcome::Unknown,
                    }],
                    occlusion: Some("partial_occlusion".to_owned()),
                },
            ],
            untested_predicates: vec!["lab.plate_misaligned.v1".to_owned()],
        },
        splits: EvaluationSplits {
            tuning_time_blocks: vec![TimeBlock {
                source_id: source.as_str().to_owned(),
                track_id: track.as_str().to_owned(),
                start_utc: stamps[0].to_string(),
                end_utc: stamps[2].to_string(),
                start_us: stamps[0].as_micros(),
                end_us: stamps[2].as_micros().saturating_add(1_000_000),
            }],
            evaluation_time_blocks: vec![TimeBlock {
                source_id: source.as_str().to_owned(),
                track_id: track.as_str().to_owned(),
                start_utc: stamps[3].to_string(),
                end_utc: stamps[4].to_string(),
                start_us: stamps[3].as_micros(),
                end_us: stamps[4].as_micros().saturating_add(1_000_000),
            }],
            adjacent_frame_exclusion_us: 120_000_000,
        },
    }
}

async fn build_deterministic_store() -> (PathBuf, EvaluationManifest) {
    let dir = temp_root("manifest");
    let _ = std::fs::remove_dir_all(&dir);
    let (store, source, track, stamps) = seed_deterministic_dvr(&dir).await;
    let manifest = build_deterministic_manifest(&store, &source, &track, stamps).await;
    (dir, manifest)
}

#[tokio::test]
async fn semantic_eval_validate_deterministic_fixture() {
    let (dir, manifest) = build_deterministic_store().await;
    let report = validate_manifest(&manifest, &dir).await.unwrap();
    assert!(report.is_success(), "warnings: {:?}", report.warnings);
    assert_eq!(report.sources_available, 1);
    assert_eq!(report.labeled_frames_verified, 2);
    assert_eq!(report.untested_predicates, vec!["lab.plate_misaligned.v1"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn semantic_eval_skips_expired_evidence_without_panic() {
    let (dir, mut manifest) = build_deterministic_store().await;
    manifest.retained_sources[0].evidence_availability = EvidenceAvailability::Expired;
    let report = validate_manifest(&manifest, &dir).await.unwrap();
    assert_eq!(report.sources_skipped, 1);
    assert_eq!(report.sources_available, 0);
    assert!(report.warnings.iter().any(|line| line.contains("skipped")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn chupacabra_pilot_template_parses() {
    let bytes = include_bytes!("../fixtures/semantic_eval/chupacabra-pilot-template-v1.json");
    let manifest = EvaluationManifest::parse_json(bytes).unwrap();
    assert_eq!(manifest.schema_version, 1);
    assert!(manifest.dvr_access.read_only);
    assert_eq!(manifest.labeled_sample.untested_predicates.len(), 3);
}
