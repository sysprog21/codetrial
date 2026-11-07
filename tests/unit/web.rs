use super::*;
use crate::recording::{Snapshot, SnapshotView};

fn missing() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "code": "recording_not_found" }),
    )
}

async fn answer(view: SnapshotView) -> (StatusCode, Value) {
    let response = replay_response(Ok(view), missing, "test");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// The history route answers a deletion it can see before reading, so this arm
/// is reached only when the recording is deleted between that check and the
/// read. It still owes the same `410` the history route gives.
#[tokio::test]
async fn a_replay_deleted_under_the_read_is_gone_not_missing() {
    let (status, body) = answer(SnapshotView::Deleted).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["code"], "recording_deleted");
}

#[tokio::test]
async fn the_replay_body_says_whether_the_replay_stopped_at_its_quota() {
    for quota_exceeded in [false, true] {
        let (status, body) = answer(SnapshotView::Ready(Snapshot {
            seq: 4,
            events: Vec::new(),
            quota_exceeded,
        }))
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({ "seq": 4, "quotaExceeded": quota_exceeded, "events": [] })
        );
    }
}
