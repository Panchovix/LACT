//! Versioned JSON IPC only; native controls and persistent baselines live in the helper.
use crate::config::NvidiaRuntimeHelperConfig;
use anyhow::{Context, bail, ensure};
use lact_schema::{NvidiaRailControls, NvidiaRuntimeDevice, NvidiaRuntimeInfo};
use serde::Serialize;
use std::{
    fs,
    io::Read,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, oneshot},
    time::timeout,
};

const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

pub(super) struct RuntimeControls {
    config: NvidiaRuntimeHelperConfig,
    device: NvidiaRuntimeDevice,
    lock: Arc<Mutex<()>>,
    pending: Mutex<Option<RuntimeTransaction>>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Status,
    Apply,
    Restore,
    Stage,
    Commit,
    Rollback,
}

#[derive(Serialize)]
struct Request<'a> {
    protocol_version: u32,
    action: Action,
    device: &'a NvidiaRuntimeDevice,
    settings: Option<&'a NvidiaRailControls>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transaction_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RuntimeTransaction {
    device: NvidiaRuntimeDevice,
    kind: TransactionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TransactionKind {
    Noop,
    LegacyDeferred(Option<NvidiaRailControls>),
    Profile {
        transaction_id: Option<String>,
        request_id: String,
    },
}

impl RuntimeTransaction {
    pub(super) fn is_staged(&self) -> bool {
        matches!(
            self.kind,
            TransactionKind::Profile {
                transaction_id: Some(_),
                ..
            }
        )
    }
}

impl RuntimeControls {
    pub(super) fn new(config: NvidiaRuntimeHelperConfig, device: NvidiaRuntimeDevice) -> Self {
        Self {
            config,
            device,
            lock: Arc::new(Mutex::new(())),
            pending: Mutex::new(None),
        }
    }

    pub(super) fn validate(&self, settings: Option<&NvidiaRailControls>) -> anyhow::Result<()> {
        self.config.validate()?;
        ensure!(
            !self.device.uuid.is_empty(),
            "NVML UUID unavailable for NVIDIA runtime controls"
        );
        validate_settings(settings)?;
        checked_executable(&self.config.executable)?;
        Ok(())
    }

    pub(super) async fn status(&self) -> NvidiaRuntimeInfo {
        match self.invoke(Action::Status, None).await {
            Ok(info) => info,
            Err(err) => NvidiaRuntimeInfo {
                protocol_version: PROTOCOL_VERSION,
                recovery_required: false,
                capabilities: Default::default(),
                transaction_id: None,
                transaction_status: None,
                request_id: None,
                telemetry: None,
                available: false,
                reason: Some(format!("{err:#}")),
                device: self.device.clone(),
                managed: false,
                rails: vec![],
            },
        }
    }

    pub(super) async fn apply(&self, settings: Option<&NvidiaRailControls>) -> anyhow::Result<()> {
        let action = if settings.is_some() {
            Action::Apply
        } else {
            Action::Restore
        };
        let info = self.invoke(action, settings).await?;
        if settings.is_some() {
            ensure!(
                info.available,
                "NVIDIA runtime controls unavailable: {}",
                info.reason
                    .as_deref()
                    .unwrap_or("helper reported unavailable")
            );
        } else {
            ensure!(
                !info.managed,
                "NVIDIA runtime helper did not release managed state"
            );
        }
        Ok(())
    }

    /// A checkpoint from another controller instance needs frequency discovery
    /// before an explicit release can lower its rail policies.
    pub(super) async fn has_unknown_profile_transaction(&self) -> anyhow::Result<bool> {
        let info = self.invoke(Action::Status, None).await?;
        if info.transaction_id.is_none() || !profile_pending(&info) {
            return Ok(false);
        }
        ensure!(
            info.capabilities.profile_transactions == 1,
            "Unsupported profile recovery capability"
        );
        validate_token(info.transaction_id.as_deref().unwrap())?;
        validate_token(
            info.request_id
                .as_deref()
                .context("Missing profile recovery request identity")?,
        )?;
        let pending = self.pending.lock().await;
        Ok(!pending
            .as_ref()
            .is_some_and(|transaction| match &transaction.kind {
                TransactionKind::Profile { request_id, .. } => {
                    info.request_id.as_deref() == Some(request_id.as_str())
                }
                _ => false,
            }))
    }

    /// Resolve a previous interrupted profile before touching boost or clocks.
    /// An unknown checkpoint requires an explicit request to release rail controls.
    pub(super) async fn recover_pending(&self, explicit_restore: bool) -> anyhow::Result<()> {
        let mut pending = self.pending.lock().await.clone();
        if let Some(transaction) = pending.as_mut()
            && let TransactionKind::Profile {
                transaction_id,
                request_id,
            } = &mut transaction.kind
            && transaction_id.is_none()
        {
            let info = self.invoke(Action::Status, None).await?;
            if info.request_id.as_deref() == Some(request_id.as_str()) {
                let id = info
                    .transaction_id
                    .context("Missing interrupted profile receipt")?;
                validate_token(&id)?;
                *transaction_id = Some(id);
                *self.pending.lock().await = Some(transaction.clone());
            } else if info.transaction_id.is_none() {
                self.clear_pending(transaction).await;
                pending = None;
            } else {
                bail!("A different NVIDIA profile transaction requires recovery");
            }
        }
        if let Some(transaction) = pending {
            let receipt = self.transaction_status(&transaction).await?;
            if matches!(
                receipt.transaction_status.as_deref(),
                Some("committed" | "rolled_back")
            ) {
                self.clear_pending(&transaction).await;
            } else {
                self.rollback(&transaction).await?;
            }
        }
        let info = self.invoke(Action::Status, None).await?;
        if profile_pending(&info) {
            ensure!(
                explicit_restore,
                "An unfinished NVIDIA profile transaction requires restoring rail controls before applying settings"
            );
            let id = info
                .transaction_id
                .context("Missing profile recovery receipt")?;
            validate_token(&id)?;
            let request_id = info
                .request_id
                .context("Missing profile recovery request identity")?;
            validate_token(&request_id)?;
            let transaction = RuntimeTransaction {
                device: self.device.clone(),
                kind: TransactionKind::Profile {
                    transaction_id: Some(id),
                    request_id,
                },
            };
            *self.pending.lock().await = Some(transaction.clone());
            self.rollback(&transaction).await?;
        } else if info.recovery_required || (!info.available && info.managed) {
            ensure!(
                explicit_restore,
                "NVIDIA runtime recovery is required before changing GPU settings"
            );
            self.apply(None).await?;
            let restored = self.invoke(Action::Status, None).await?;
            ensure!(
                !restored.recovery_required && !restored.managed,
                "NVIDIA runtime recovery remains pending"
            );
        }
        Ok(())
    }

    pub(super) async fn stage(
        &self,
        settings: Option<&NvidiaRailControls>,
    ) -> anyhow::Result<RuntimeTransaction> {
        self.validate(settings)?;
        ensure!(
            self.pending.lock().await.is_none(),
            "A previous NVIDIA profile transaction still needs recovery"
        );
        let info = self.invoke(Action::Status, None).await?;
        if info.capabilities.profile_transactions != 1 {
            // Preserve older providers: apply them last, as before this extension.
            return Ok(RuntimeTransaction {
                device: self.device.clone(),
                kind: TransactionKind::LegacyDeferred(settings.cloned()),
            });
        }
        ensure!(
            info.transaction_id.is_none(),
            "An unfinished NVIDIA profile transaction needs recovery"
        );
        let request_id = new_request_id()?;
        let pending = RuntimeTransaction {
            device: self.device.clone(),
            kind: TransactionKind::Profile {
                transaction_id: None,
                request_id: request_id.clone(),
            },
        };
        *self.pending.lock().await = Some(pending);
        let result = async {
            let info = self
                .invoke_with_ids(Action::Stage, settings, None, Some(&request_id))
                .await?;
            ensure!(
                info.request_id.as_deref() == Some(request_id.as_str()),
                "NVIDIA stage returned a different request identity"
            );
            if info.transaction_status.as_deref() == Some("noop") {
                ensure!(
                    settings.is_none_or(|settings| *settings == NvidiaRailControls::default())
                        && info.transaction_id.is_none()
                        && !info.managed
                        && !info.recovery_required,
                    "NVIDIA helper discarded a nonempty or managed profile request as a noop"
                );
                *self.pending.lock().await = None;
                return Ok(RuntimeTransaction {
                    device: self.device.clone(),
                    kind: TransactionKind::Noop,
                });
            }
            ensure!(
                info.transaction_status.as_deref() == Some("staged"),
                "NVIDIA helper did not stage the profile"
            );
            let id = info
                .transaction_id
                .context("NVIDIA helper did not return a profile receipt")?;
            validate_token(&id)?;
            let transaction = RuntimeTransaction {
                device: self.device.clone(),
                kind: TransactionKind::Profile {
                    transaction_id: Some(id),
                    request_id: request_id.clone(),
                },
            };
            *self.pending.lock().await = Some(transaction.clone());
            ensure!(
                settings.is_none() || info.available,
                "NVIDIA runtime controls unavailable after stage: {}",
                info.reason.as_deref().unwrap_or("unknown reason")
            );
            Ok(transaction)
        }
        .await;
        match result {
            Ok(transaction) => Ok(transaction),
            Err(error) => match self.recover_pending(false).await {
                Ok(()) => {
                    Err(error.context("NVIDIA profile stage failed; its checkpoint was recovered"))
                }
                Err(recovery) => Err(error.context(format!(
                    "NVIDIA profile stage failed and recovery remains pending: {recovery:#}"
                ))),
            },
        }
    }

    pub(super) async fn commit(&self, transaction: &RuntimeTransaction) -> anyhow::Result<()> {
        ensure!(
            transaction.device == self.device,
            "NVIDIA profile receipt belongs to a different GPU"
        );
        match &transaction.kind {
            TransactionKind::Noop => Ok(()),
            TransactionKind::LegacyDeferred(settings) => self.apply(settings.as_ref()).await,
            TransactionKind::Profile { .. } => {
                self.finalize(transaction, Action::Commit, "committed")
                    .await
            }
        }
    }

    pub(super) async fn rollback(&self, transaction: &RuntimeTransaction) -> anyhow::Result<()> {
        ensure!(
            transaction.device == self.device,
            "NVIDIA profile receipt belongs to a different GPU"
        );
        match &transaction.kind {
            TransactionKind::Noop | TransactionKind::LegacyDeferred(_) => Ok(()),
            TransactionKind::Profile { .. } => {
                self.finalize(transaction, Action::Rollback, "rolled_back")
                    .await
            }
        }
    }

    async fn transaction_status(
        &self,
        transaction: &RuntimeTransaction,
    ) -> anyhow::Result<NvidiaRuntimeInfo> {
        let TransactionKind::Profile {
            transaction_id,
            request_id,
        } = &transaction.kind
        else {
            bail!("Not a native profile transaction")
        };
        let id = transaction_id
            .as_deref()
            .context("Missing profile transaction id")?;
        let info = self
            .invoke_with_ids(Action::Status, None, Some(id), None)
            .await?;
        ensure!(
            info.transaction_id.as_deref() == Some(id)
                && info.request_id.as_deref() == Some(request_id.as_str()),
            "NVIDIA profile status returned a different receipt"
        );
        Ok(info)
    }

    async fn finalize(
        &self,
        transaction: &RuntimeTransaction,
        action: Action,
        expected: &str,
    ) -> anyhow::Result<()> {
        let TransactionKind::Profile {
            transaction_id,
            request_id,
        } = &transaction.kind
        else {
            bail!("Not a native profile transaction")
        };
        let id = transaction_id
            .as_deref()
            .context("Missing profile transaction id")?;
        let result = self.invoke_with_ids(action, None, Some(id), None).await;
        if result.as_ref().is_ok_and(|info| {
            info.transaction_id.as_deref() == Some(id)
                && info.request_id.as_deref() == Some(request_id.as_str())
                && info.transaction_status.as_deref() == Some(expected)
        }) {
            self.clear_pending(transaction).await;
            return Ok(());
        }
        // A finalization may have completed even if its stdout was lost.
        if self
            .transaction_status(transaction)
            .await
            .is_ok_and(|info| info.transaction_status.as_deref() == Some(expected))
        {
            self.clear_pending(transaction).await;
            return Ok(());
        }
        match result {
            Err(error) => Err(error.context("NVIDIA profile finalization is unconfirmed")),
            Ok(info) => bail!(
                "NVIDIA profile finalization is unconfirmed: {:?}",
                info.transaction_status
            ),
        }
    }

    async fn clear_pending(&self, transaction: &RuntimeTransaction) {
        let mut pending = self.pending.lock().await;
        if pending.as_ref() == Some(transaction) {
            *pending = None;
        }
    }

    async fn invoke(
        &self,
        action: Action,
        settings: Option<&NvidiaRailControls>,
    ) -> anyhow::Result<NvidiaRuntimeInfo> {
        self.invoke_with_ids(action, settings, None, None).await
    }

    async fn invoke_with_ids(
        &self,
        action: Action,
        settings: Option<&NvidiaRailControls>,
        transaction_id: Option<&str>,
        request_id: Option<&str>,
    ) -> anyhow::Result<NvidiaRuntimeInfo> {
        self.validate(settings)?;
        for token in [transaction_id, request_id].into_iter().flatten() {
            validate_token(token)?;
        }
        let guard = self.lock.clone().lock_owned().await;
        let config = self.config.clone();
        let device = self.device.clone();
        let settings = settings.cloned();
        let transaction_id = transaction_id.map(str::to_owned);
        let request_id = request_id.map(str::to_owned);
        let (tx, rx) = oneshot::channel();
        // Cancellation must not unlock the GPU while its child is still running.
        tokio::spawn(async move {
            let _guard = guard;
            let result =
                invoke_process(config, device, action, settings, transaction_id, request_id).await;
            let _ = tx.send(result);
        });
        rx.await
            .context("NVIDIA runtime helper task was interrupted")?
    }
}

fn profile_pending(info: &NvidiaRuntimeInfo) -> bool {
    matches!(
        info.transaction_status.as_deref(),
        Some("prepared" | "staged" | "rollback_required")
    )
}

fn validate_token(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Invalid NVIDIA profile transaction identifier"
    );
    Ok(())
}

fn new_request_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn invoke_process(
    config: NvidiaRuntimeHelperConfig,
    device: NvidiaRuntimeDevice,
    action: Action,
    settings: Option<NvidiaRailControls>,
    transaction_id: Option<String>,
    request_id: Option<String>,
) -> anyhow::Result<NvidiaRuntimeInfo> {
    let executable = checked_executable(&config.executable)?;
    let request = serde_json::to_vec(&Request {
        protocol_version: PROTOCOL_VERSION,
        action,
        device: &device,
        settings: settings.as_ref(),
        transaction_id: transaction_id.as_deref(),
        request_id: request_id.as_deref(),
    })?;
    let mut command = Command::new(executable);
    command.as_std_mut().process_group(0);
    command
        .arg("lact-bridge")
        .arg("--state-dir")
        .arg(&config.state_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("Could not start NVIDIA runtime helper")?;
    let mut process_group = ProcessGroup(child.id().context("Missing helper process ID")?);
    let mut stdin = child.stdin.take().context("Missing helper stdin")?;
    let stdout = child.stdout.take().context("Missing helper stdout")?;
    let stderr = child.stderr.take().context("Missing helper stderr")?;
    let operation = async {
        let write = async {
            stdin
                .write_all(&request)
                .await
                .context("Could not write helper request")?;
            stdin
                .shutdown()
                .await
                .context("Could not close helper request")?;
            drop(stdin);
            Ok::<_, anyhow::Error>(())
        };
        let ((), stdout, stderr, status) =
            tokio::try_join!(write, bounded_read(stdout), bounded_read(stderr), async {
                child
                    .wait()
                    .await
                    .context("Could not wait for runtime helper")
            },)?;
        ensure!(
            status.success(),
            "NVIDIA runtime helper failed ({status}): {}",
            String::from_utf8_lossy(&stderr)
        );
        let info: NvidiaRuntimeInfo =
            serde_json::from_slice(&stdout).context("Invalid NVIDIA runtime helper JSON")?;
        ensure!(
            info.protocol_version == PROTOCOL_VERSION,
            "Unsupported NVIDIA runtime helper protocol {}",
            info.protocol_version
        );
        ensure!(
            info.device == device,
            "NVIDIA runtime helper returned a different GPU identity"
        );
        Ok(info)
    };
    let result = timeout(Duration::from_millis(config.timeout_ms), operation).await;
    match result {
        Ok(Ok(info)) => {
            process_group.0 = 0;
            Ok(info)
        }
        failure => {
            // Reap the child before releasing serialization. kill_on_drop also covers cancellation.
            process_group.terminate();
            let _ = timeout(Duration::from_secs(1), child.kill()).await;
            match failure {
                Ok(Err(err)) => Err(err),
                Err(_) => bail!(
                    "NVIDIA runtime helper timed out after {} ms",
                    config.timeout_ms
                ),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

/// Keep descendants from retaining IPC pipes after failure or cancellation.
struct ProcessGroup(u32);

impl ProcessGroup {
    fn terminate(&mut self) {
        let group = std::mem::take(&mut self.0);
        if group != 0
            && let Ok(pid) = i32::try_from(group)
        {
            // SAFETY: the child creates its own process group, identified by its PID.
            unsafe {
                nix::libc::kill(-pid, nix::libc::SIGKILL);
            }
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn checked_executable(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical =
        fs::canonicalize(path).context("Could not resolve NVIDIA runtime executable")?;
    let uid = nix::unistd::geteuid().as_raw();
    for (index, component) in canonical.ancestors().enumerate() {
        let metadata =
            fs::metadata(component).context("Could not inspect NVIDIA runtime executable path")?;
        ensure!(
            metadata.uid() == 0 || metadata.uid() == uid,
            "NVIDIA runtime executable path must be owned by root or the daemon user: {}",
            component.display()
        );
        // A root-owned sticky ancestor such as /tmp protects the independently checked child.
        let sticky_root_directory =
            metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        ensure!(
            metadata.mode() & 0o022 == 0 || sticky_root_directory,
            "NVIDIA runtime executable path is writable by other users: {}",
            component.display()
        );
        if index == 0 {
            ensure!(
                metadata.is_file() && metadata.mode() & 0o111 != 0,
                "NVIDIA runtime executable must be an executable regular file"
            );
        } else {
            ensure!(
                metadata.is_dir(),
                "NVIDIA runtime executable ancestor must be a directory"
            );
        }
    }
    Ok(canonical)
}

async fn bounded_read(reader: impl AsyncRead + Unpin) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .context("Could not read runtime helper output")?;
    ensure!(
        bytes.len() <= MAX_OUTPUT_BYTES,
        "NVIDIA runtime helper output exceeds {MAX_OUTPUT_BYTES} bytes"
    );
    Ok(bytes)
}

pub(super) fn validate_settings(settings: Option<&NvidiaRailControls>) -> anyhow::Result<()> {
    let Some(settings) = settings else {
        return Ok(());
    };
    for rail in [&settings.nvvdd, &settings.msvdd] {
        for value in [
            rail.min_voltage_uv,
            rail.max_voltage_uv,
            rail.overvoltage_uv,
            rail.ocp_limit_ma,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(
                value > 0,
                "NVIDIA runtime settings must be positive integers"
            );
        }
        if let Some(min) = rail.min_voltage_uv {
            for upper in [rail.max_voltage_uv, rail.overvoltage_uv]
                .into_iter()
                .flatten()
            {
                ensure!(
                    min <= upper,
                    "NVIDIA runtime minimum voltage exceeds an upper policy"
                );
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    use tempfile::TempDir;

    pub(super) const RESPONSE: &str = r#"{"protocol_version":1,"available":true,"device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},"managed":false,"rails":[]}"#;

    pub(super) fn fake(body: &str, timeout_ms: u64) -> (TempDir, RuntimeControls) {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-helper");
        fs::write(&executable, format!("#!/usr/bin/python3\nimport sys,json,os,time\nr=json.load(sys.stdin)\nassert sys.argv[1:3] == ['lact-bridge','--state-dir']\nstate_dir=sys.argv[3]\n{body}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = RuntimeControls::new(
            NvidiaRuntimeHelperConfig {
                executable,
                state_dir: dir.path().to_owned(),
                timeout_ms,
            },
            NvidiaRuntimeDevice {
                uuid: "GPU-test".into(),
                bdf: "0000:09:00.0".into(),
            },
        );
        (dir, runtime)
    }

    #[tokio::test]
    async fn status_and_apply_restore_use_only_typed_json() {
        let body = format!(
            "with open(os.path.join(state_dir,'requests'),'a') as f: f.write(json.dumps(r)+'\\n')\nprint({RESPONSE:?})"
        );
        let (dir, runtime) = fake(&body, 2000);
        assert!(runtime.status().await.available);
        let mut settings = NvidiaRailControls::default();
        settings.nvvdd.min_voltage_uv = Some(900_125);
        runtime.apply(Some(&settings)).await.unwrap();
        runtime.apply(None).await.unwrap();
        let requests: Vec<serde_json::Value> = fs::read_to_string(dir.path().join("requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0],
            serde_json::json!({"protocol_version":1,"action":"status","device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},"settings":null})
        );
        assert_eq!(requests[1]["action"], "apply");
        assert_eq!(
            requests[1]["settings"],
            serde_json::json!({"nvvdd":{"min_voltage_uv":900_125},"msvdd":{}})
        );
        assert_eq!(requests[2]["action"], "restore");
        assert!(requests[2]["settings"].is_null());
    }

    #[tokio::test]
    async fn explicit_restore_does_not_require_in_memory_touched_state() {
        let body = format!(
            "assert r['action']=='restore'\nwith open(os.path.join(state_dir,'restored'),'w') as f: f.write('original')\nprint({RESPONSE:?})"
        );
        let (dir, runtime) = fake(&body, 2000);
        runtime.apply(None).await.unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("restored")).unwrap(),
            "original"
        );
    }

    #[tokio::test]
    async fn validates_protocol_and_both_identity_components() {
        for response in [
            RESPONSE.replace("\"protocol_version\":1", "\"protocol_version\":2"),
            RESPONSE.replace("GPU-test", "GPU-other"),
            RESPONSE.replace("0000:09:00.0", "0000:0a:00.0"),
            "not json".into(),
        ] {
            let (_dir, runtime) = fake(&format!("print({response:?})"), 2000);
            assert!(
                runtime
                    .apply(Some(&NvidiaRailControls::default()))
                    .await
                    .is_err()
            );
            assert!(!runtime.status().await.available);
        }
    }

    #[tokio::test]
    async fn unavailable_status_is_success_but_requested_apply_fails() {
        let response = RESPONSE.replace("\"available\":true", "\"available\":false");
        let (_dir, runtime) = fake(&format!("print({response:?})"), 2000);
        assert!(!runtime.status().await.available);
        assert!(
            runtime
                .apply(Some(&NvidiaRailControls::default()))
                .await
                .is_err()
        );
        runtime.apply(None).await.unwrap();
        let response = RESPONSE.replace("\"managed\":false", "\"managed\":true");
        let (_dir, runtime) = fake(&format!("print({response:?})"), 2000);
        assert!(runtime.apply(None).await.is_err());
    }

    #[tokio::test]
    async fn nonzero_exit_propagates_and_missing_helper_is_unavailable() {
        let (_dir, mut runtime) = fake(
            "sys.stderr.write('{\"error\":\"rollback unconfirmed\"}')\nsys.exit(2)",
            2000,
        );
        assert!(
            runtime
                .apply(None)
                .await
                .unwrap_err()
                .to_string()
                .contains("rollback unconfirmed")
        );
        runtime.config.executable = runtime.config.state_dir.join("does-not-exist");
        assert!(!runtime.status().await.available);
        assert!(runtime.apply(None).await.is_err());
    }

    #[tokio::test]
    async fn rejects_oversized_stdout_and_stderr() {
        for stream in ["stdout", "stderr"] {
            let (_dir, runtime) = fake(
                &format!("sys.{stream}.write('x'*65537)\nsys.{stream}.flush()\ntime.sleep(30)"),
                2000,
            );
            assert!(
                runtime
                    .apply(None)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("exceeds")
            );
        }
    }

    #[tokio::test]
    async fn timeout_terminates_and_reaps_child() {
        let (dir, runtime) = fake(
            "with open(os.path.join(state_dir,'pid'),'w') as f: f.write(str(os.getpid()))\ntime.sleep(30)",
            1000,
        );
        assert!(
            runtime
                .apply(None)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        let pid = fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(!std::path::Path::new("/proc").join(pid).exists());
    }

    #[tokio::test]
    async fn calls_are_serialized_per_gpu() {
        let body = format!(
            "with open(os.path.join(state_dir,'calls'),'a') as f: f.write('start\\n')\ntime.sleep(0.05)\nwith open(os.path.join(state_dir,'calls'),'a') as f: f.write('end\\n')\nprint({RESPONSE:?})"
        );
        let (dir, runtime) = fake(&body, 2000);
        let (one, two) = tokio::join!(runtime.apply(None), runtime.apply(None));
        one.unwrap();
        two.unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("calls")).unwrap(),
            "start\nend\nstart\nend\n"
        );
    }

    #[tokio::test]
    async fn cancelled_caller_cannot_unlock_before_child_completes_and_is_reaped() {
        let body = format!(
            "import socket\nwith open(os.path.join(state_dir,'calls'),'a') as f: f.write('start\\n')\nwith open(os.path.join(state_dir,'pids'),'a') as f: f.write(str(os.getpid())+'\\n')\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)\ns.connect(os.path.join(state_dir,'gate'))\ns.sendall(b'x')\nassert s.recv(1)==b'x'\ns.close()\nwith open(os.path.join(state_dir,'calls'),'a') as f: f.write('end\\n')\nprint({RESPONSE:?})"
        );
        let (dir, runtime) = fake(&body, 5000);
        let listener = tokio::net::UnixListener::bind(dir.path().join("gate")).unwrap();
        let runtime = Arc::new(runtime);
        let first_runtime = runtime.clone();
        let first = tokio::spawn(async move { first_runtime.apply(None).await });
        let (mut first_gate, _) = listener.accept().await.unwrap();
        first_gate.read_u8().await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let mut second = Box::pin(runtime.apply(None));
        assert!(futures::poll!(second.as_mut()).is_pending());
        let mut next_gate = Box::pin(listener.accept());
        assert!(futures::poll!(next_gate.as_mut()).is_pending());
        first_gate.write_all(b"x").await.unwrap();
        let (result, ()) = tokio::join!(second, async {
            let (mut second_gate, _) = next_gate.await.unwrap();
            second_gate.read_u8().await.unwrap();
            second_gate.write_all(b"x").await.unwrap();
        });
        result.unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("calls")).unwrap(),
            "start\nend\nstart\nend\n"
        );
        for pid in fs::read_to_string(dir.path().join("pids")).unwrap().lines() {
            assert!(!Path::new("/proc").join(pid).exists());
        }
    }

    #[tokio::test]
    async fn invalid_config_and_settings_never_start_helper() {
        let (dir, mut runtime) = fake("open(os.path.join(state_dir,'started'),'w').close()", 2000);
        let mut settings = NvidiaRailControls::default();
        settings.msvdd.ocp_limit_ma = Some(0);
        assert!(runtime.apply(Some(&settings)).await.is_err());
        settings.msvdd.ocp_limit_ma = None;
        settings.nvvdd.min_voltage_uv = Some(1001);
        settings.nvvdd.max_voltage_uv = Some(1000);
        assert!(runtime.apply(Some(&settings)).await.is_err());
        runtime.config.executable = "relative-helper".into();
        assert!(runtime.apply(None).await.is_err());
        assert!(!dir.path().join("started").exists());
    }

    #[test]
    fn executable_and_parent_must_not_be_writable_by_other_users() {
        let (_dir, runtime) = fake("pass", 2000);
        assert!(checked_executable(&runtime.config.executable).is_ok());
        fs::set_permissions(
            &runtime.config.executable,
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(checked_executable(&runtime.config.executable).is_err());
        fs::set_permissions(
            &runtime.config.executable,
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(&runtime.config.state_dir, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(checked_executable(&runtime.config.executable).is_err());
    }

    #[test]
    fn helper_is_disabled_by_default_and_equal_min_max_is_allowed() {
        assert!(crate::config::Daemon::default().nvidia_runtime.is_none());
        let mut settings = NvidiaRailControls::default();
        settings.nvvdd.min_voltage_uv = Some(900_000);
        settings.nvvdd.max_voltage_uv = Some(900_000);
        assert!(validate_settings(Some(&settings)).is_ok());
        let config: NvidiaRuntimeHelperConfig = serde_json::from_str(
            r#"{"executable":"/usr/libexec/helper","state_dir":"/var/lib/helper"}"#,
        )
        .unwrap();
        assert_eq!(config.timeout_ms, 10_000);
    }
}

#[cfg(all(test, not(miri)))]
#[path = "runtime_controls_profile_tests.rs"]
mod profile_tests;
