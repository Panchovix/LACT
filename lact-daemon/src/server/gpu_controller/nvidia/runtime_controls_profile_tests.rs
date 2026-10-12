use super::tests::{RESPONSE, fake};
use super::*;

fn profile_fake() -> (tempfile::TempDir, RuntimeControls) {
    fake(include_str!("runtime_controls_profile_helper.py"), 3000)
}

fn requested() -> NvidiaRailControls {
    let mut settings = NvidiaRailControls::default();
    settings.nvvdd.max_voltage_uv = Some(950_000);
    settings
}

fn actions(dir: &tempfile::TempDir) -> Vec<String> {
    fs::read_to_string(dir.path().join("calls"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn legacy_provider_defers_application_until_commit() {
    let body = format!(
        "with open(os.path.join(state_dir,'calls'),'a') as f: f.write(r['action']+'\\n')\nprint({RESPONSE:?})"
    );
    let (dir, runtime) = fake(&body, 2000);
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    assert!(!transaction.is_staged());
    assert_eq!(actions(&dir), ["status"]);
    runtime.rollback(&transaction).await.unwrap();
    assert_eq!(actions(&dir), ["status"]);
    runtime.commit(&transaction).await.unwrap();
    assert_eq!(actions(&dir), ["status", "apply"]);
}

#[tokio::test]
async fn stage_commit_and_read_receipt_are_correlated() {
    let (dir, runtime) = profile_fake();
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    assert!(transaction.is_staged());
    assert!(!runtime.has_unknown_profile_transaction().await.unwrap());
    assert!(runtime.pending.lock().await.is_some());
    runtime.commit(&transaction).await.unwrap();
    runtime.commit(&transaction).await.unwrap();
    assert!(runtime.pending.lock().await.is_none());
    assert_eq!(
        actions(&dir)
            .iter()
            .filter(|action| *action == "stage")
            .count(),
        1
    );
}

#[tokio::test]
async fn lost_stage_reply_recovers_only_its_own_checkpoint() {
    let (dir, runtime) = profile_fake();
    fs::write(dir.path().join("mode"), "lost_stage_reply").unwrap();
    assert!(runtime.stage(Some(&requested())).await.is_err());
    assert!(runtime.pending.lock().await.is_none());
    assert!(actions(&dir).iter().any(|action| action == "rollback"));
    assert!(!runtime.status().await.recovery_required);
}

#[tokio::test]
async fn lost_commit_reply_is_confirmed_without_restoring_or_restaging() {
    let (dir, runtime) = profile_fake();
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    fs::write(dir.path().join("mode"), "lost_commit_reply").unwrap();
    runtime.commit(&transaction).await.unwrap();
    assert!(runtime.pending.lock().await.is_none());
    assert!(!actions(&dir).iter().any(|action| action == "rollback"));
}

#[tokio::test]
async fn rollback_failure_blocks_next_apply_until_recovery_succeeds() {
    let (dir, runtime) = profile_fake();
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    fs::write(dir.path().join("mode"), "rollback_failure").unwrap();
    assert!(runtime.rollback(&transaction).await.is_err());
    assert!(runtime.recover_pending(false).await.is_err());
    assert!(runtime.pending.lock().await.is_some());
    fs::write(dir.path().join("mode"), "").unwrap();
    runtime.recover_pending(false).await.unwrap();
    assert!(runtime.pending.lock().await.is_none());
}

#[tokio::test]
async fn unknown_profile_requires_explicit_release_and_frequency_discovery() {
    let (dir, runtime) = profile_fake();
    let _transaction = runtime.stage(Some(&requested())).await.unwrap();
    let restarted = RuntimeControls::new(runtime.config.clone(), runtime.device.clone());
    assert!(restarted.has_unknown_profile_transaction().await.unwrap());
    assert!(restarted.recover_pending(false).await.is_err());
    assert!(!actions(&dir).iter().any(|action| action == "rollback"));
    restarted.recover_pending(true).await.unwrap();
    assert!(!restarted.has_unknown_profile_transaction().await.unwrap());
}

#[tokio::test]
async fn noop_cannot_discard_a_nonempty_request() {
    let (dir, runtime) = profile_fake();
    fs::write(dir.path().join("mode"), "bad_noop").unwrap();
    assert!(runtime.stage(Some(&requested())).await.is_err());
    let noop = runtime.stage(None).await.unwrap();
    assert!(!noop.is_staged());
    runtime.commit(&noop).await.unwrap();
}

#[tokio::test]
async fn receipt_request_identity_is_checked_on_finalization() {
    let (dir, runtime) = profile_fake();
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    fs::write(dir.path().join("mode"), "wrong_receipt_request").unwrap();
    assert!(runtime.commit(&transaction).await.is_err());
    assert!(runtime.pending.lock().await.is_some());
    fs::write(dir.path().join("mode"), "").unwrap();
    runtime.recover_pending(false).await.unwrap();
    assert!(runtime.pending.lock().await.is_none());
}

#[tokio::test]
async fn native_pending_without_profile_receipt_still_blocks_mutations() {
    let (dir, runtime) = profile_fake();
    fs::write(dir.path().join("mode"), "native_pending_without_receipt").unwrap();
    assert!(runtime.stage(Some(&requested())).await.is_err());
    assert!(runtime.recover_pending(false).await.is_err());
    assert!(runtime.status().await.recovery_required);
    runtime.recover_pending(true).await.unwrap();
    assert!(!runtime.status().await.recovery_required);
}
