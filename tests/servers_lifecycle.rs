//! Integration test for the storage::servers reaper.

use std::time::Duration;

use rover::storage::Db;

#[tokio::test]
async fn reap_after_threshold_removes_stale_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Db::open(tmp.path().join("rover.db")).await.unwrap();

    db.upsert_server_self(11, "0.1.0".into()).await.unwrap();
    // Wait longer than the threshold and reap with a tiny threshold.
    // `now_epoch` is in whole seconds, so we need >= threshold + 1s to
    // guarantee `last_heartbeat < cutoff` regardless of sub-second timing.
    tokio::time::sleep(Duration::from_millis(2200)).await;
    let removed = db.reap_stale_servers(Duration::from_secs(1)).await.unwrap();
    assert_eq!(removed, 1);
    assert!(db.list_servers().await.unwrap().is_empty());
}
