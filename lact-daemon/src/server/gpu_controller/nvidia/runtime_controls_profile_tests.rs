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
    let supported = RESPONSE.replace("\"rails\":[]", "\"rails\":[{\"rail\":\"NVVDD\",\"max_voltage\":{\"evaluated_uv\":950000,\"offset_uv\":0,\"writable\":true}}]");
    let body = format!(
        "with open(os.path.join(state_dir,'calls'),'a') as f: f.write(r['action']+'\\n')\nprint({supported:?})"
    );
    let (dir, runtime) = fake(&body, 2000);
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    assert!(!transaction.is_staged());
    assert_eq!(actions(&dir), ["status", "status"]);
    runtime.rollback(&transaction).await.unwrap();
    assert_eq!(actions(&dir), ["status", "status"]);
    runtime.commit(&transaction).await.unwrap();
    assert_eq!(actions(&dir), ["status", "status", "apply"]);
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
    let restarted = runtime.restarted();
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

/// The whole Apply precondition, not only recover_pending, keeps an unfinished
/// change of ours fail-closed, even when validation or the first status fail.
#[tokio::test]
async fn own_unfinished_change_keeps_every_apply_precondition_fail_closed() {
    let (dir, mut runtime) = profile_fake();
    let _staged = runtime.stage(Some(&requested())).await.unwrap();
    let executable = runtime.config.executable.clone();
    runtime.config.executable = runtime.config.state_dir.join("missing-helper");
    for settings in [None, Some(requested())] {
        let error = runtime
            .prepare_apply(settings.as_ref(), |_| panic!("validation failed first"))
            .await
            .unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
    }
    runtime.config.executable = executable;
    fs::write(dir.path().join("mode"), "status_failure").unwrap();
    let error = runtime
        .prepare_apply(None, |_| panic!("the unknown-checkpoint status failed"))
        .await
        .unwrap_err();
    assert!(!is_unavailable(&error), "{error:#}");
    assert!(!is_unavailable(
        &runtime.recover(true, |_| Ok(())).await.unwrap_err()
    ));
    // A GPU reload replaces the controller but keeps what it knew.
    let reloaded = runtime.reloaded();
    let error = reloaded.prepare_apply(None, |_| Ok(())).await.unwrap_err();
    assert!(!is_unavailable(&error), "{error:#}");
    // A new daemon knows nothing, so the same outage degrades there.
    let restarted = runtime.restarted();
    let error = restarted.prepare_apply(None, |_| Ok(())).await.unwrap_err();
    assert!(is_unavailable(&error), "{error:#}");
}

#[tokio::test]
async fn recovery_reported_by_a_valid_reply_survives_later_helper_failures() {
    let (dir, runtime) = profile_fake();
    let _staged = runtime.stage(Some(&requested())).await.unwrap();
    let mut restarted = runtime.restarted();
    // The first status reports another instance's checkpoint; the next one fails.
    fs::write(dir.path().join("mode"), "status_once").unwrap();
    let error = restarted
        .prepare_apply(None, |unknown| {
            assert!(unknown);
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(!is_unavailable(&error), "{error:#}");
    // Later Applies stay fail-closed while the helper keeps failing.
    let error = restarted.prepare_apply(None, |_| Ok(())).await.unwrap_err();
    assert!(!is_unavailable(&error), "{error:#}");
    // A valid reply without pending recovery allows degrading on a later outage.
    fs::write(dir.path().join("mode"), "").unwrap();
    restarted.prepare_apply(None, |_| Ok(())).await.unwrap();
    restarted.config.executable = restarted.config.state_dir.join("missing-helper");
    let error = restarted.prepare_apply(None, |_| Ok(())).await.unwrap_err();
    assert!(is_unavailable(&error), "{error:#}");
}

#[tokio::test]
async fn unknown_checkpoint_recovers_frequencies_before_any_rail_rollback() {
    let (dir, runtime) = profile_fake();
    let _staged = runtime.stage(Some(&requested())).await.unwrap();
    let restarted = runtime.restarted();
    let mut discovered = Vec::new();
    restarted
        .prepare_apply(None, |unknown| {
            assert!(!actions(&dir).iter().any(|action| action == "rollback"));
            discovered.push(unknown);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(discovered, [true]);
    assert!(actions(&dir).iter().any(|action| action == "rollback"));
    // Nothing unknown remains, so the next preparation needs no discovery.
    restarted
        .prepare_apply(None, |unknown| {
            assert!(!unknown);
            Ok(())
        })
        .await
        .unwrap();
}

/// Only an exited helper without any reported or own unfinished transaction
/// lets the controller apply its independent settings.
#[tokio::test]
async fn reported_or_own_unfinished_recovery_is_never_unavailability() {
    let (dir, runtime) = profile_fake();
    let staged = runtime.stage(Some(&requested())).await.unwrap();
    let restarted = runtime.restarted();
    let reported = restarted.recover_pending(false).await.unwrap_err();
    assert!(!is_unavailable(&reported), "{reported:#}");
    fs::write(dir.path().join("mode"), "rollback_failure").unwrap();
    let ours = runtime.recover_pending(false).await.unwrap_err();
    assert!(!is_unavailable(&ours), "{ours:#}");
    let mut stopped = runtime.restarted();
    *stopped.pending.lock().await = Some(staged);
    stopped.config.executable = stopped.config.state_dir.join("missing-helper");
    let unanswered = stopped.recover_pending(false).await.unwrap_err();
    assert!(!is_unavailable(&unanswered), "{unanswered:#}");

    let (dir, runtime) = profile_fake();
    fs::write(dir.path().join("mode"), "native_pending_without_receipt").unwrap();
    assert!(runtime.stage(Some(&requested())).await.is_err());
    fs::write(dir.path().join("mode"), "restore_failure").unwrap();
    let native = runtime.recover_pending(true).await.unwrap_err();
    assert!(!is_unavailable(&native), "{native:#}");
    assert!(runtime.status().await.recovery_required);
}
