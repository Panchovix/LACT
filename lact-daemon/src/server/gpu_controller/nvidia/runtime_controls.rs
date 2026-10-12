//! Versioned JSON IPC only; native controls and persistent baselines live in the helper.
use crate::config::NvidiaRuntimeHelperConfig;
use anyhow::{Context, anyhow, bail, ensure};
use lact_schema::{NvidiaRailControls, NvidiaRuntimeDevice, NvidiaRuntimeInfo};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc, LazyLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{Mutex, oneshot},
    time::timeout,
};
use tracing::error;

const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// A killed helper still blocked in the driver keeps its native state unknown.
const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// No valid typed response arrived and no helper process remains, so rail
/// state is unknown but nothing can still be writing it.
#[derive(Debug)]
struct HelperUnavailable(anyhow::Error);

/// A failure while native recovery may be pending: a valid response reported
/// it, a call that could write got no valid response, or a killed helper has
/// not exited. The helper verifies unrelated CONTROL then, so other groups
/// must still wait.
#[derive(Debug)]
struct RecoveryReported(anyhow::Error);

/// The helper ran and refused the request with its typed error, for example
/// because another helper process holds this GPU's lock.
#[derive(Debug)]
struct HelperRefused(anyhow::Error);

macro_rules! transparent_error {
    ($name:ident) => {
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl std::error::Error for $name {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.0.source()
            }
        }
    };
}

transparent_error!(HelperUnavailable);
transparent_error!(RecoveryReported);
transparent_error!(HelperRefused);

pub(super) fn unavailable(error: anyhow::Error) -> anyhow::Error {
    HelperUnavailable(error).into()
}

fn recovery_reported(error: anyhow::Error) -> anyhow::Error {
    RecoveryReported(error).into()
}

/// Whether only the independent setting groups may still be applied.
pub(super) fn is_unavailable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<HelperUnavailable>())
        && !error.chain().any(|cause| cause.is::<RecoveryReported>())
}

pub(super) struct RuntimeControls {
    config: NvidiaRuntimeHelperConfig,
    device: NvidiaRuntimeDevice,
    guard: Arc<DeviceGuard>,
    pending: Mutex<Option<RuntimeTransaction>>,
    reap_timeout: Duration,
}

/// Helper state shared by every controller instance of one GPU, so a reload
/// keeps serializing helper processes and cannot forget unresolved recovery or
/// a killed helper that has not exited.
#[derive(Default)]
struct DeviceGuard {
    lock: Arc<Mutex<()>>,
    /// Set from the start of a call that could write and by a valid reply that
    /// reports recovery; cleared only by a valid reply without recovery. Only
    /// the call holding `lock` updates it, so a reply cannot clear a later
    /// call's mark.
    recovery_outstanding: AtomicBool,
    /// A killed helper may still be inside a native call.
    unreaped: AtomicBool,
}

/// Keyed by GPU UUID and BDF.
type DeviceGuards = std::sync::Mutex<HashMap<(String, String), Arc<DeviceGuard>>>;

fn device_guard(device: &NvidiaRuntimeDevice) -> Arc<DeviceGuard> {
    static GUARDS: LazyLock<DeviceGuards> = LazyLock::new(Default::default);
    GUARDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((device.uuid.clone(), device.bdf.clone()))
        .or_default()
        .clone()
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

impl Action {
    /// Whether an unconfirmed call may have changed native helper state.
    fn may_write(self) -> bool {
        !matches!(self, Self::Status)
    }
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
        let guard = device_guard(&device);
        Self::with_guard(config, device, guard)
    }

    fn with_guard(
        config: NvidiaRuntimeHelperConfig,
        device: NvidiaRuntimeDevice,
        guard: Arc<DeviceGuard>,
    ) -> Self {
        Self {
            config,
            device,
            guard,
            pending: Mutex::new(None),
            reap_timeout: REAP_TIMEOUT,
        }
    }

    /// Another controller instance of the same GPU, as after a GPU reload.
    #[cfg(test)]
    fn reloaded(&self) -> Self {
        Self::with_guard(self.config.clone(), self.device.clone(), self.guard.clone())
    }

    /// The same GPU in a newly started daemon, which remembers nothing.
    #[cfg(test)]
    fn restarted(&self) -> Self {
        Self::with_guard(self.config.clone(), self.device.clone(), Arc::default())
    }

    pub(super) fn validate(&self, settings: Option<&NvidiaRailControls>) -> anyhow::Result<()> {
        validate_settings(settings)?;
        self.ensure_runnable()
    }

    /// Configuration and installation problems leave the helper unusable.
    fn ensure_runnable(&self) -> anyhow::Result<()> {
        let runnable = || {
            self.config.validate()?;
            ensure!(
                !self.device.uuid.is_empty(),
                "NVML UUID unavailable for NVIDIA runtime controls"
            );
            checked_executable(&self.config.executable)?;
            anyhow::Ok(())
        };
        runnable().map_err(unavailable)
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

    /// An unfinished change of this controller, recovery outstanding before or
    /// during `operation`, or a killed helper that has not exited may have left
    /// native helper state behind, so any failure then stays fail-closed, even
    /// one that validation reports before any helper call.
    async fn guard_own_pending<T>(
        &self,
        operation: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let ours = self.pending.lock().await.is_some();
        operation.await.map_err(|error| {
            if ours
                || self.guard.recovery_outstanding.load(Ordering::SeqCst)
                || self.guard.unreaped.load(Ordering::SeqCst)
            {
                recovery_reported(error)
            } else {
                error
            }
        })
    }

    /// Everything an Apply needs from the helper before writing any group: a
    /// usable helper and resolved recovery. `recover_frequencies` receives
    /// whether an unknown checkpoint needs frequency discovery and runs before
    /// any rail rollback.
    pub(super) async fn prepare_apply(
        &self,
        settings: Option<&NvidiaRailControls>,
        recover_frequencies: impl FnOnce(bool) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.guard_own_pending(async {
            self.validate(settings)?;
            self.recover_unguarded(settings.is_none(), recover_frequencies)
                .await
        })
        .await
    }

    /// Resolve recovery before a reset or an explicit release, as for an Apply.
    pub(super) async fn recover(
        &self,
        explicit_restore: bool,
        recover_frequencies: impl FnOnce(bool) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.guard_own_pending(self.recover_unguarded(explicit_restore, recover_frequencies))
            .await
    }

    async fn recover_unguarded(
        &self,
        explicit_restore: bool,
        recover_frequencies: impl FnOnce(bool) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let unknown = explicit_restore && self.has_unknown_profile_transaction().await?;
        recover_frequencies(unknown)?;
        self.recover_pending_inner(explicit_restore).await
    }

    /// Resolve a previous interrupted profile before touching boost or clocks.
    /// An unknown checkpoint requires an explicit request to release rail controls.
    pub(super) async fn recover_pending(&self, explicit_restore: bool) -> anyhow::Result<()> {
        self.guard_own_pending(self.recover_pending_inner(explicit_restore))
            .await
    }

    async fn recover_pending_inner(&self, explicit_restore: bool) -> anyhow::Result<()> {
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
            self.rollback(&transaction)
                .await
                .map_err(recovery_reported)?;
        } else if info.recovery_required || (!info.available && info.managed) {
            ensure!(
                explicit_restore,
                "NVIDIA runtime recovery is required before changing GPU settings"
            );
            async {
                self.apply(None).await?;
                let restored = self.invoke(Action::Status, None).await?;
                ensure!(
                    !restored.recovery_required && !restored.managed,
                    "NVIDIA runtime recovery remains pending"
                );
                anyhow::Ok(())
            }
            .await
            .map_err(recovery_reported)?;
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
        let lock = self.guard.lock.clone().lock_owned().await;
        // A killed helper may still be inside a native call; never start another.
        ensure!(
            !self.guard.unreaped.load(Ordering::SeqCst),
            "A killed NVIDIA runtime helper has not exited yet"
        );
        let guard = self.guard.clone();
        let config = self.config.clone();
        let device = self.device.clone();
        let settings = settings.cloned();
        let transaction_id = transaction_id.map(str::to_owned);
        let request_id = request_id.map(str::to_owned);
        let reap_timeout = self.reap_timeout;
        let (tx, rx) = oneshot::channel();
        // A cancelled caller can neither unlock the GPU while its child is still
        // running nor lose what the outcome means for recovery: the task records
        // it under the lock before handing the result over.
        tokio::spawn(async move {
            let _lock = lock;
            if action.may_write() {
                // Native state stays unknown until a valid reply says otherwise.
                guard.recovery_outstanding.store(true, Ordering::SeqCst);
            }
            let request = HelperRequest {
                action,
                settings,
                transaction_id,
                request_id,
            };
            let (result, running) = invoke_process(config, device, request, reap_timeout).await;
            if let Some(mut child) = running {
                guard.unreaped.store(true, Ordering::SeqCst);
                tokio::spawn(reap(guard.clone(), async move { child.wait().await }));
            }
            if let Ok(info) = &result {
                guard
                    .recovery_outstanding
                    .store(reports_recovery(info), Ordering::SeqCst);
            }
            let _ = tx.send(result);
        });
        rx.await
            .context("NVIDIA runtime helper task was interrupted")
            .and_then(|result| result)
    }
}

/// Only a confirmed exit unblocks the GPU: after a failed wait, the killed
/// helper may still be inside a native call.
async fn reap(guard: Arc<DeviceGuard>, exit: impl Future<Output = std::io::Result<ExitStatus>>) {
    match exit.await {
        Ok(_) => guard.unreaped.store(false, Ordering::SeqCst),
        Err(err) => error!(
            "could not reap a killed NVIDIA runtime helper, its GPU stays blocked until lactd restarts: {err}"
        ),
    }
}

fn profile_pending(info: &NvidiaRuntimeInfo) -> bool {
    matches!(
        info.transaction_status.as_deref(),
        Some("prepared" | "staged" | "rollback_required")
    )
}

/// Each of these needs recovery before any other profile group, as handled by
/// `recover_pending`.
fn reports_recovery(info: &NvidiaRuntimeInfo) -> bool {
    profile_pending(info) || info.recovery_required || (!info.available && info.managed)
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

/// The owned request of one helper process.
struct HelperRequest {
    action: Action,
    settings: Option<NvidiaRailControls>,
    transaction_id: Option<String>,
    request_id: Option<String>,
}

/// Also returns the child when it was killed but did not exit within
/// `reap_timeout`, so that no other helper runs until it does.
async fn invoke_process(
    config: NvidiaRuntimeHelperConfig,
    device: NvidiaRuntimeDevice,
    request: HelperRequest,
    reap_timeout: Duration,
) -> (anyhow::Result<NvidiaRuntimeInfo>, Option<Child>) {
    // Nothing can write native state before a helper process exists.
    let prepare = || {
        let executable = checked_executable(&config.executable)?;
        let payload = serde_json::to_vec(&Request {
            protocol_version: PROTOCOL_VERSION,
            action: request.action,
            device: &device,
            settings: request.settings.as_ref(),
            transaction_id: request.transaction_id.as_deref(),
            request_id: request.request_id.as_deref(),
        })?;
        anyhow::Ok((executable, payload))
    };
    let (executable, payload) = match prepare() {
        Ok(prepared) => prepared,
        Err(err) => return (Err(unavailable(err)), None),
    };
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
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let err = anyhow::Error::new(err).context("Could not start NVIDIA runtime helper");
            return (Err(unavailable(err)), None);
        }
    };
    let mut process_group = ProcessGroup(child.id().unwrap_or(0));
    let operation = async {
        let mut stdin = child.stdin.take().context("Missing helper stdin")?;
        let stdout = child.stdout.take().context("Missing helper stdout")?;
        let stderr = child.stderr.take().context("Missing helper stderr")?;
        let write = async {
            stdin
                .write_all(&payload)
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
        if !status.success() {
            let error = anyhow!(
                "NVIDIA runtime helper failed ({status}): {}",
                String::from_utf8_lossy(&stderr)
            );
            return Err(if typed_refusal(&stderr) {
                HelperRefused(error).into()
            } else {
                error
            });
        }
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
            (Ok(info), None)
        }
        failure => {
            // Reap the child before releasing serialization. kill_on_drop also covers cancellation.
            process_group.terminate();
            let reaped = timeout(reap_timeout, async {
                let _ = child.start_kill();
                child.wait().await
            })
            .await
            .is_ok_and(|status| status.is_ok());
            let error = match failure {
                Ok(Err(err)) => err,
                Err(_) => anyhow!(
                    "NVIDIA runtime helper timed out after {} ms",
                    config.timeout_ms
                ),
                Ok(Ok(_)) => unreachable!(),
            };
            if !reaped {
                // It may still be inside a native call, so rails stay unknown.
                let error = error.context("NVIDIA runtime helper did not exit after being killed");
                return (Err(error), Some(child));
            }
            // A typed refusal means the helper ran and judged its own state.
            if error.chain().any(|cause| cause.is::<HelperRefused>()) {
                (Err(error), None)
            } else {
                (Err(unavailable(error)), None)
            }
        }
    }
}

/// The bridge reports its own errors as a single JSON object on stderr.
fn typed_refusal(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .is_some_and(|value| {
            value.get("protocol_version") == Some(&serde_json::json!(PROTOCOL_VERSION))
                && value.get("error").is_some_and(serde_json::Value::is_string)
        })
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
    use std::{
        fs,
        os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    };
    use tempfile::TempDir;

    pub(super) const RESPONSE: &str = r#"{"protocol_version":1,"available":true,"device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},"managed":false,"rails":[]}"#;

    pub(super) fn fake(body: &str, timeout_ms: u64) -> (TempDir, RuntimeControls) {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake-helper");
        let source = dir.path().join("fake-helper.py");
        fs::write(&source, format!("#!/usr/bin/python3\nimport sys,json,os,time\nr=json.load(sys.stdin)\nassert sys.argv[1:3] == ['lact-bridge','--state-dir']\nstate_dir=sys.argv[3]\n{body}\n")).unwrap();
        // Another test's fork inherits our write descriptors until it execs, and
        // executing a file still open for writing fails with ETXTBSY. A child
        // process writes the executable, so this process never holds it open.
        let installed = std::process::Command::new("install")
            .arg("-m700")
            .arg(&source)
            .arg(&executable)
            .status()
            .unwrap();
        assert!(installed.success());
        // A guard of its own keeps parallel tests from sharing helper state.
        let runtime = RuntimeControls::with_guard(
            NvidiaRuntimeHelperConfig {
                executable,
                state_dir: dir.path().to_owned(),
                timeout_ms,
            },
            NvidiaRuntimeDevice {
                uuid: "GPU-test".into(),
                bdf: "0000:09:00.0".into(),
            },
            Arc::default(),
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

    #[tokio::test]
    async fn failures_without_a_typed_response_from_an_exited_helper_are_unavailable() {
        let (_dir, mut runtime) = fake("pass", 2000);
        runtime.config.executable = runtime.config.state_dir.join("missing-helper");
        let missing = runtime.validate(None).unwrap_err();
        assert!(is_unavailable(&missing), "{missing:#}");
        assert!(is_unavailable(
            &runtime.has_unknown_profile_transaction().await.unwrap_err()
        ));
        for (body, timeout_ms) in [
            // A broken interpreter environment, unusable output and a hang.
            ("sys.stderr.write('ModuleNotFoundError')\nsys.exit(1)", 2000),
            ("print('not json')", 2000),
            ("time.sleep(30)", 300),
        ] {
            let (_dir, runtime) = fake(body, timeout_ms);
            let error = runtime.stage(None).await.unwrap_err();
            assert!(is_unavailable(&error), "{error:#}");
            assert!(is_unavailable(&error.context("caller context")));
        }
        let (_dir, runtime) = fake("pass", 2000);
        let mut settings = NvidiaRailControls::default();
        settings.nvvdd.min_voltage_uv = Some(0);
        let invalid = runtime.validate(Some(&settings)).unwrap_err();
        assert!(!is_unavailable(&invalid));
    }

    #[test]
    fn unavailability_keeps_the_original_message() {
        let error = unavailable(anyhow!("inner cause").context("outer message"));
        assert_eq!(error.to_string(), "outer message");
        assert_eq!(format!("{error:#}"), "outer message: inner cause");
        // Reported recovery overrides an unavailability found deeper in the chain.
        let reported = recovery_reported(error.context("caller"));
        assert!(
            reported
                .chain()
                .any(|cause| cause.is::<HelperUnavailable>())
        );
        assert!(!is_unavailable(&reported));
    }

    #[tokio::test]
    async fn typed_helper_refusal_is_never_unavailability() {
        // The bridge refuses while another helper process holds this GPU's lock.
        let (_dir, runtime) = fake(
            "sys.stderr.write(json.dumps({'protocol_version':1,'error':'ValidationError','message':'Another runtime request holds this GPU lock'})+'\\n')\nsys.exit(2)",
            2000,
        );
        let error = runtime.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        assert!(format!("{error:#}").contains("holds this GPU lock"));
    }

    #[tokio::test]
    async fn killed_helper_that_has_not_exited_blocks_every_later_call() {
        let (dir, runtime) = fake(
            &format!("open(os.path.join(state_dir,'started'),'a').close()\nprint({RESPONSE:?})"),
            2000,
        );
        runtime.guard.unreaped.store(true, Ordering::SeqCst);
        let error = runtime.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        assert!(
            runtime
                .reloaded()
                .prepare_apply(None, |_| Ok(()))
                .await
                .is_err()
        );
        // Nothing else is pending, and the missing executable fails validation
        // before any call could notice the killed helper.
        let mut missing = runtime.reloaded();
        missing.config.executable = missing.config.state_dir.join("missing-helper");
        assert!(missing.pending.lock().await.is_none());
        assert!(!missing.guard.recovery_outstanding.load(Ordering::SeqCst));
        let error = missing.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        assert!(!dir.path().join("started").exists());
        runtime.guard.unreaped.store(false, Ordering::SeqCst);
        runtime.prepare_apply(None, |_| Ok(())).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_wait_never_unblocks_a_killed_helper() {
        let (dir, runtime) = fake(
            &format!("open(os.path.join(state_dir,'started'),'a').close()\nprint({RESPONSE:?})"),
            2000,
        );
        runtime.guard.unreaped.store(true, Ordering::SeqCst);
        reap(runtime.guard.clone(), async {
            Err(std::io::Error::other("wait failed"))
        })
        .await;
        assert!(runtime.apply(None).await.is_err());
        assert!(!dir.path().join("started").exists());
        reap(runtime.guard.clone(), async { Ok(ExitStatus::from_raw(0)) }).await;
        runtime.apply(None).await.unwrap();
        assert!(dir.path().join("started").exists());
    }

    #[tokio::test]
    async fn failed_writes_keep_recovery_outstanding_until_a_clean_status() {
        let (dir, runtime) = fake(
            &format!(
                "mode_path=os.path.join(state_dir,'mode')\nif os.path.exists(mode_path) and open(mode_path).read()=='fail': sys.exit(1)\nprint({RESPONSE:?})"
            ),
            2000,
        );
        let mode = |value: &str| fs::write(dir.path().join("mode"), value).unwrap();
        mode("fail");
        // A restore may have written before it failed without a reply.
        assert!(runtime.apply(None).await.is_err());
        let error = runtime.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        let reloaded = runtime.reloaded();
        let error = reloaded.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
        // A clean status clears it, so a later outage may degrade again.
        mode("");
        runtime.prepare_apply(None, |_| Ok(())).await.unwrap();
        mode("fail");
        let error = runtime.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(is_unavailable(&error), "{error:#}");
    }

    /// Holds the helper until the test answers on the `gate` socket.
    const GATE: &str = "import socket\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)\ns.connect(os.path.join(state_dir,'gate'))\ns.sendall(b'x')\nassert s.recv(1)==b'x'\ns.close()";

    /// A cancelled caller cannot drop what its helper's outcome means for a
    /// later outage. Without degrading, an Apply returns before writing any
    /// independent group.
    #[tokio::test]
    async fn cancelled_write_still_decides_whether_a_later_outage_may_degrade() {
        let recovery = RESPONSE.replace("\"rails\"", "\"recovery_required\":true,\"rails\"");
        for (outcome, degrades) in [
            ("sys.exit(1)".to_owned(), false),
            (format!("print({recovery:?})"), false),
            (format!("print({RESPONSE:?})"), true),
        ] {
            let (dir, runtime) = fake(&format!("{GATE}\n{outcome}"), 5000);
            let listener = tokio::net::UnixListener::bind(dir.path().join("gate")).unwrap();
            let runtime = Arc::new(runtime);
            let caller = runtime.clone();
            let call = tokio::spawn(async move { caller.apply(None).await });
            let (mut gate, _) = listener.accept().await.unwrap();
            gate.read_u8().await.unwrap();
            call.abort();
            assert!(call.await.unwrap_err().is_cancelled());
            gate.write_all(b"x").await.unwrap();
            // The helper's task holds the GPU lock until it has finished.
            drop(runtime.guard.lock.lock().await);
            let mut reloaded = runtime.reloaded();
            reloaded.config.executable = reloaded.config.state_dir.join("missing-helper");
            let error = reloaded.prepare_apply(None, |_| Ok(())).await.unwrap_err();
            assert_eq!(is_unavailable(&error), degrades, "{outcome}: {error:#}");
        }
    }

    /// Outcomes are recorded in the order the helpers ran, not in the order
    /// their callers resume.
    #[tokio::test]
    async fn an_earlier_clean_reply_cannot_clear_a_later_failed_write() {
        let (dir, runtime) = fake(
            &format!(
                "if open(os.path.join(state_dir,'mode')).read()=='fail': sys.exit(1)\nprint({RESPONSE:?})"
            ),
            2000,
        );
        let mode = |value: &str| fs::write(dir.path().join("mode"), value).unwrap();
        mode("");
        let mut earlier = Box::pin(runtime.status());
        assert!(futures::poll!(earlier.as_mut()).is_pending());
        // Its helper finishes, but its caller does not resume yet.
        drop(runtime.guard.lock.lock().await);
        mode("fail");
        assert!(runtime.apply(None).await.is_err());
        assert!(earlier.await.available);
        let mut reloaded = runtime.reloaded();
        reloaded.config.executable = reloaded.config.state_dir.join("missing-helper");
        let error = reloaded.prepare_apply(None, |_| Ok(())).await.unwrap_err();
        assert!(!is_unavailable(&error), "{error:#}");
    }
}

#[cfg(all(test, not(miri)))]
#[path = "runtime_controls_profile_tests.rs"]
mod profile_tests;
