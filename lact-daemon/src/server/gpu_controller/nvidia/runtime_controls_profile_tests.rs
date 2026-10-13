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

fn modern_profile_fake() -> (tempfile::TempDir, RuntimeControls) {
    let body = include_str!("runtime_controls_profile_helper.py")
        .replace("\"profile_transactions\": 1", "\"profile_transactions\": 1, \"rail_probe\": 1")
        .replace("if r[\"action\"] == \"stage\":", "if r[\"action\"] == \"probe\":\n    if state[\"checkpoint\"]: emit(state[\"checkpoint\"], True)\n    base.update(can_stage=True)\n    emit()\nif r[\"action\"] == \"stage\":");
    fake(&body, 3000)
}

#[tokio::test]
async fn prepared_modern_none_and_some_use_one_status_and_one_probe() {
    for settings in [None, Some(requested())] {
        let (dir, runtime) = modern_profile_fake();
        let prepared = runtime
            .prepare_apply(settings.as_ref(), |unknown| {
                assert!(!unknown);
                Ok(())
            })
            .await
            .unwrap();
        assert!(prepared.modern_ticket.is_some());
        assert_eq!(actions(&dir), ["status", "probe"]);
        let transaction = runtime
            .stage_prepared(settings.as_ref(), prepared)
            .await
            .unwrap();
        runtime.commit(&transaction).await.unwrap();
        let expected = if settings.is_some() {
            vec!["status", "probe", "stage", "commit"]
        } else {
            vec!["status", "probe", "stage"]
        };
        assert_eq!(actions(&dir), expected);
        assert!(runtime.pending.lock().await.is_none());
        assert!(!runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn prepared_ticket_exec_swap_and_other_controller_use_conservative_preflight() {
    for swap in [false, true] {
        let (dir, runtime) = modern_profile_fake();
        let settings = requested();
        let prepared = runtime
            .prepare_apply(Some(&settings), |_| Ok(()))
            .await
            .unwrap();
        let receiver = if swap { &runtime } else { &runtime.reloaded() };
        if swap {
            let replacement = dir.path().join("replacement");
            assert!(
                std::process::Command::new("install")
                    .args(["-m700"])
                    .arg(&runtime.config.executable)
                    .arg(&replacement)
                    .status()
                    .unwrap()
                    .success()
            );
            fs::rename(replacement, &runtime.config.executable).unwrap();
        }
        let transaction = receiver
            .stage_prepared(Some(&settings), prepared)
            .await
            .unwrap();
        receiver.commit(&transaction).await.unwrap();
        assert_eq!(
            actions(&dir),
            [
                "status", "probe", "status", "probe", "status", "stage", "commit"
            ]
        );
    }
}

#[tokio::test]
async fn prepared_ticket_changed_settings_requires_new_probe() {
    let (dir, runtime) = modern_profile_fake();
    let mut settings = requested();
    let prepared = runtime
        .prepare_apply(Some(&settings), |_| Ok(()))
        .await
        .unwrap();
    settings.nvvdd.max_voltage_uv = Some(975000);
    let transaction = runtime
        .stage_prepared(Some(&settings), prepared)
        .await
        .unwrap();
    runtime.commit(&transaction).await.unwrap();
    assert_eq!(
        actions(&dir),
        [
            "status", "probe", "status", "probe", "status", "stage", "commit"
        ]
    );
}

#[tokio::test]
async fn prepared_ticket_cannot_bypass_own_pending_or_shared_recovery() {
    for condition in 0..3 {
        let (dir, runtime) = modern_profile_fake();
        let settings = requested();
        let prepared = runtime
            .prepare_apply(Some(&settings), |_| Ok(()))
            .await
            .unwrap();
        if condition == 0 {
            *runtime.pending.lock().await = Some(RuntimeTransaction {
                device: runtime.device.clone(),
                kind: TransactionKind::Profile {
                    transaction_id: None,
                    request_id: "a".repeat(64),
                },
            });
        } else if condition == 1 {
            runtime
                .guard
                .recovery_outstanding
                .store(true, Ordering::SeqCst);
        } else {
            runtime.guard.unreaped.store(true, Ordering::SeqCst);
        }
        let error = runtime
            .stage_prepared(Some(&settings), prepared)
            .await
            .unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        assert_eq!(actions(&dir), ["status", "probe"]);
        if condition == 0 {
            assert!(runtime.pending.lock().await.is_some());
        }
        if condition == 1 {
            assert!(runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
        }
        if condition == 2 {
            assert!(runtime.guard.unreaped.load(Ordering::SeqCst));
        }
    }
}

#[tokio::test]
async fn prepared_unknown_recovery_refreshes_status_after_frequency_then_rollback() {
    let (dir, runtime) = modern_profile_fake();
    let transaction = runtime.stage(Some(&requested())).await.unwrap();
    let restarted = runtime.restarted();
    fs::write(dir.path().join("calls"), "").unwrap();
    let blocked = restarted
        .prepare_apply(None, |unknown| {
            assert!(unknown);
            anyhow::bail!("synthetic frequency compensation failure")
        })
        .await
        .unwrap_err();
    assert!(!is_unavailable(&blocked));
    assert_eq!(actions(&dir), ["status"]);
    let prepared = restarted
        .prepare_apply(None, |unknown| {
            assert!(unknown);
            assert!(!actions(&dir).iter().any(|action| action == "rollback"));
            fs::write(dir.path().join("frequency-compensated"), "yes").unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert!(dir.path().join("frequency-compensated").exists());
    assert_eq!(
        actions(&dir),
        ["status", "status", "rollback", "status", "probe"]
    );
    assert!(prepared.modern_ticket.is_some());
    let noop = restarted.stage_prepared(None, prepared).await.unwrap();
    restarted.commit(&noop).await.unwrap();
    assert_eq!(actions(&dir).last().unwrap(), "stage");
    assert!(restarted.pending.lock().await.is_none());
    assert!(!noop.is_staged());
    // The original receipt was resolved rather than replaced or re-estimated.
    assert_eq!(
        restarted
            .transaction_status(&transaction)
            .await
            .unwrap()
            .transaction_status
            .as_deref(),
        Some("rolled_back")
    );
}

#[tokio::test]
async fn local_frequency_failure_precedes_every_helper_rpc_when_not_releasing() {
    let (dir, runtime) = modern_profile_fake();
    let error = runtime
        .prepare_apply(Some(&requested()), |unknown| {
            assert!(!unknown);
            assert!(!dir.path().join("calls").exists());
            anyhow::bail!("synthetic local frequency recovery failure")
        })
        .await
        .unwrap_err();
    assert!(!is_unavailable(&error));
    assert!(!dir.path().join("calls").exists());
}

#[tokio::test]
async fn disappeared_executable_defers_prepared_rails_only_without_pending_recovery() {
    for pending in [false, true] {
        let (dir, runtime) = modern_profile_fake();
        let settings = requested();
        let prepared = runtime
            .prepare_apply(Some(&settings), |_| Ok(()))
            .await
            .unwrap();
        fs::rename(
            &runtime.config.executable,
            dir.path().join("removed-helper"),
        )
        .unwrap();
        if pending {
            runtime
                .guard
                .recovery_outstanding
                .store(true, Ordering::SeqCst);
        }
        let result = runtime.stage_prepared(Some(&settings), prepared).await;
        if pending {
            let error = result.unwrap_err();
            assert!(!is_unavailable(&error), "{error:#}");
            assert!(runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
        } else {
            let transaction = result.unwrap();
            assert!(transaction.deferral().is_some());
            runtime.commit(&transaction).await.unwrap();
            assert!(!runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
        }
        assert_eq!(actions(&dir), ["status", "probe"]);
        assert!(runtime.pending.lock().await.is_none());
    }
}

fn metadata_profile_fake() -> (tempfile::TempDir, RuntimeControls) {
    let body = include_str!("runtime_controls_profile_helper.py")
        .replace("\"profile_transactions\": 1", "\"profile_transactions\": 1, \"rail_probe\": 1, \"metadata_status\": 1")
        .replace("if r[\"action\"] == \"stage\":", "if r[\"action\"] == \"probe\":\n    if state[\"checkpoint\"]: emit(state[\"checkpoint\"], True)\n    base.update(can_stage=True)\n    emit()\nif r[\"action\"] == \"stage\":")
        .replace("if r[\"action\"] == \"status\":", r#"if r["action"] == "metadata":
    assert r.get("settings") is None
    if mode in ("metadata_busy", "metadata_busy_and_status_failure"):
        sys.stderr.write(json.dumps(dict(protocol_version=1, error="BusyError", message="synthetic busy")))
        sys.exit(2)
    if mode == "metadata_failure":
        sys.stderr.write(json.dumps(dict(protocol_version=1, error="UnsupportedError", message="synthetic unsupported metadata")))
        sys.exit(2)
    if mode == "metadata_cap_mismatch":
        base["capabilities"]["metadata_status"] = 0
    if state["checkpoint"]: emit(state["checkpoint"], True)
    base["rails"] = []
    if mode == "metadata_clean_owned":
        base.update(available=False, managed=True, ownership_unavailable=True)
    emit()
if r["action"] == "status":"#)
        .replace("if mode == \"status_failure\":", "if mode in (\"status_failure\", \"metadata_busy_and_status_failure\"):");
    fake(&body, 3000)
}

#[tokio::test]
async fn metadata_is_negotiated_from_full_status_and_keeps_policy_probe_fresh() {
    for settings in [None, Some(requested())] {
        let (dir, runtime) = metadata_profile_fake();
        for iteration in 0..2 {
            let prepared = runtime
                .prepare_apply(settings.as_ref(), |_| Ok(()))
                .await
                .unwrap();
            let transaction = runtime
                .stage_prepared(settings.as_ref(), prepared)
                .await
                .unwrap();
            runtime.commit(&transaction).await.unwrap();
            assert!(runtime.pending.lock().await.is_none());
            assert_eq!(
                actions(&dir)
                    .iter()
                    .filter(|action| *action == "probe")
                    .count(),
                iteration + 1
            );
        }
        let expected = if settings.is_some() {
            vec![
                "status", "probe", "stage", "commit", "metadata", "probe", "stage", "commit",
            ]
        } else {
            vec!["status", "probe", "stage", "metadata", "probe", "stage"]
        };
        assert_eq!(actions(&dir), expected);
        assert!(!runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn gui_status_can_negotiate_metadata_but_executable_change_invalidates_it() {
    let (dir, runtime) = metadata_profile_fake();
    assert!(runtime.status().await.available);
    assert!(runtime.metadata_hint.lock().await.is_some());
    assert!(runtime.recovery_status().await.unwrap().rails.is_empty());
    assert_eq!(actions(&dir), ["status", "metadata"]);
    let replacement = dir.path().join("replacement");
    assert!(
        std::process::Command::new("install")
            .args(["-m700"])
            .arg(&runtime.config.executable)
            .arg(&replacement)
            .status()
            .unwrap()
            .success()
    );
    fs::rename(replacement, &runtime.config.executable).unwrap();
    assert!(!runtime.recovery_status().await.unwrap().rails.is_empty());
    assert_eq!(actions(&dir), ["status", "metadata", "status"]);
}

#[tokio::test]
async fn metadata_error_or_capability_mismatch_retries_full_status_exactly_once() {
    for mode in ["metadata_failure", "metadata_cap_mismatch", "metadata_busy"] {
        let (dir, runtime) = metadata_profile_fake();
        runtime.status().await;
        fs::write(dir.path().join("mode"), mode).unwrap();
        let info = runtime.recovery_status().await.unwrap();
        assert!(!info.rails.is_empty());
        assert_eq!(actions(&dir), ["status", "metadata", "status"]);
        assert!(!runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn refused_metadata_cannot_degrade_when_full_status_retry_also_fails() {
    let (dir, runtime) = metadata_profile_fake();
    runtime.status().await;
    fs::write(dir.path().join("mode"), "metadata_busy_and_status_failure").unwrap();
    let error = runtime
        .prepare_apply(Some(&requested()), |_| Ok(()))
        .await
        .unwrap_err();
    assert!(!is_unavailable(&error), "{error:#}");
    assert_eq!(actions(&dir), ["status", "metadata", "status"]);
    assert!(runtime.metadata_hint.lock().await.is_none());
    fs::write(dir.path().join("mode"), "").unwrap();
    runtime
        .prepare_apply(Some(&requested()), |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(&actions(&dir)[3..], ["status", "probe"]);
}

#[tokio::test]
async fn metadata_clean_unreadable_ownership_is_not_legacy_recovery() {
    let (dir, runtime) = metadata_profile_fake();
    runtime.status().await;
    fs::write(dir.path().join("mode"), "metadata_clean_owned").unwrap();
    let info = runtime.recovery_status().await.unwrap();
    assert!(!info.available && info.managed && info.ownership_unavailable);
    assert!(!reports_recovery(&info));
    assert!(!runtime.guard.recovery_outstanding.load(Ordering::SeqCst));
    assert_eq!(actions(&dir), ["status", "metadata"]);
}

#[tokio::test]
async fn pending_or_recovery_guards_never_select_metadata_and_unreaped_never_spawns() {
    for condition in 0..3 {
        let (dir, runtime) = metadata_profile_fake();
        runtime.status().await;
        if condition == 0 {
            *runtime.pending.lock().await = Some(RuntimeTransaction {
                device: runtime.device.clone(),
                kind: TransactionKind::Noop,
            });
        } else if condition == 1 {
            runtime
                .guard
                .recovery_outstanding
                .store(true, Ordering::SeqCst);
        } else {
            runtime.guard.unreaped.store(true, Ordering::SeqCst);
        }
        let result = runtime.recovery_status().await;
        if condition == 2 {
            assert!(result.is_err());
            assert_eq!(actions(&dir), ["status"]);
            assert!(runtime.guard.unreaped.load(Ordering::SeqCst));
        } else {
            assert!(result.is_ok());
            assert_eq!(actions(&dir), ["status", "status"]);
        }
    }
}

#[tokio::test]
async fn metadata_detected_unknown_checkpoint_compensates_before_rollback_and_refreshes_full() {
    let (dir, runtime) = metadata_profile_fake();
    let restarted = runtime.restarted();
    restarted.status().await;
    let staged = runtime.stage(Some(&requested())).await.unwrap();
    fs::write(dir.path().join("calls"), "").unwrap();
    let prepared = restarted
        .prepare_apply(None, |unknown| {
            assert!(unknown);
            assert_eq!(actions(&dir), ["metadata"]);
            Ok(())
        })
        .await
        .unwrap();
    assert!(prepared.modern_ticket.is_some());
    assert_eq!(actions(&dir), ["metadata", "rollback", "status", "probe"]);
    assert!(!restarted.guard.recovery_outstanding.load(Ordering::SeqCst));
    assert_eq!(
        restarted
            .transaction_status(&staged)
            .await
            .unwrap()
            .transaction_status
            .as_deref(),
        Some("rolled_back")
    );
}
