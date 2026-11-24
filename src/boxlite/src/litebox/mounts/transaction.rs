//! One runtime mount transaction across the shim records and the guest.
//!
//! Adding is register, guest mount, mark `Mounted`. Removing is mark
//! `Detaching`, guest detach by export identity, retire the export. Any
//! other outcome of the guest mount is unknown — the guest may have mounted
//! without reporting it — so it takes the removal path before the error
//! surfaces. A record leaves the shim only after the guest confirmed that its
//! identity is no longer mounted.

use std::fs::File;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use boxlite_shared::constants::mount_tags::AGGREGATE;
use boxlite_shared::errors::{BoxliteError, BoxliteResult};

use super::{MountInfo, MountRequest, MountState};
use crate::portal::GuestSession;
use crate::portal::interfaces::ContainerInterface;
use crate::vmm::controller::mount_control::{
    ControlMount, MountControlClient, MountState as RecordState, normalize_target,
};

/// How long one guest mount or detach may take before its outcome is unknown.
const GUEST_STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// The shim's mount records on `vmm.sock`. Every call blocks its thread for up
/// to the exchange budget: run it on a dedicated thread, never on an async
/// executor worker.
pub(crate) trait MountControl: Send + Sync {
    fn register(
        &self,
        source: &str,
        target: &str,
        directory: File,
        read_only: bool,
    ) -> io::Result<ControlMount>;
    fn list(&self) -> io::Result<Vec<ControlMount>>;
    fn set_state(&self, export: &str, state: RecordState) -> io::Result<()>;
    fn remove(&self, export: &str) -> io::Result<()>;
}

impl MountControl for MountControlClient {
    fn register(
        &self,
        source: &str,
        target: &str,
        directory: File,
        read_only: bool,
    ) -> io::Result<ControlMount> {
        MountControlClient::register(
            self,
            source.to_owned(),
            target.to_owned(),
            directory,
            read_only,
        )
    }

    fn list(&self) -> io::Result<Vec<ControlMount>> {
        MountControlClient::list(self)
    }

    fn set_state(&self, export: &str, state: RecordState) -> io::Result<()> {
        MountControlClient::set_state(self, export, state)
    }

    fn remove(&self, export: &str) -> io::Result<()> {
        MountControlClient::remove(self, export)
    }
}

/// The guest half of a mount: what the container's mount table holds.
#[allow(
    clippy::double_must_use,
    reason = "async_trait generates a must-use boxed future"
)]
#[async_trait]
pub(crate) trait GuestMounts: Send + Sync {
    async fn mount(&mut self, export: &str, target: &str, read_only: bool) -> BoxliteResult<()>;
    async fn detach(&mut self, export: &str, target: &str) -> BoxliteResult<()>;
}

/// The container interface as the coordinator's guest half.
struct ContainerMounts {
    interface: ContainerInterface,
    container_id: String,
}

#[async_trait]
impl GuestMounts for ContainerMounts {
    async fn mount(&mut self, export: &str, target: &str, read_only: bool) -> BoxliteResult<()> {
        self.interface
            .mount_aggregate(&self.container_id, export, AGGREGATE, target, read_only)
            .await
    }

    async fn detach(&mut self, export: &str, target: &str) -> BoxliteResult<()> {
        self.interface
            .detach(&self.container_id, target, export)
            .await
    }
}

/// What one mount operation needs from the VM run it was admitted to.
pub(crate) struct MountRun {
    pub(crate) control: Arc<MountControlClient>,
    pub(crate) guest: GuestSession,
    pub(crate) container_id: String,
    /// Cancelling it ends every transaction of this box at its next step.
    pub(crate) shutdown: CancellationToken,
    /// Whether the admitted VM run is still the live one. Export identities
    /// restart with every run, so no request may outlive the run it names.
    pub(crate) is_current: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Deliver `directory` at `request.target`. The transaction runs in a task of
/// its own: a caller that gives up cannot abandon its cleanup.
pub(crate) async fn add(
    run: MountRun,
    request: CheckedRequest,
    directory: File,
) -> BoxliteResult<MountInfo> {
    let transaction = Transaction::open(run).await?;
    let shutdown = transaction.shutdown.clone();
    owned(shutdown, async move {
        transaction.add(request, directory).await
    })
    .await
}

/// Remove the mount at `target`. Runs in a task of its own, like [`add`].
pub(crate) async fn remove(run: MountRun, target: &str) -> BoxliteResult<()> {
    let transaction = Transaction::open(run).await?;
    let shutdown = transaction.shutdown.clone();
    let target = target.to_owned();
    owned(shutdown, async move { transaction.remove(&target).await }).await
}

/// Every record of the VM run, as callers see them.
pub(crate) async fn list(control: Arc<MountControlClient>) -> BoxliteResult<Vec<MountInfo>> {
    let records = tokio::task::spawn_blocking(move || control.list())
        .await
        .map_err(|error| BoxliteError::Internal(format!("mount list task: {error}")))?
        .map_err(|error| shim_error("list the records", error))?;
    Ok(records
        .iter()
        .map(|record| caller_info(record, caller_state(record.state)))
        .collect())
}

/// Open the host source as a directory descriptor. Anything but a directory,
/// including a symbolic link, is refused before a descriptor leaves this process.
pub(crate) async fn open_source(source: &Path) -> BoxliteResult<File> {
    let path = source.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
    })
    .await
    .map_err(|error| BoxliteError::Internal(format!("source open task: {error}")))?
    .map_err(|error| match error.raw_os_error() {
        Some(libc::ENOENT) => {
            BoxliteError::NotFound(format!("source {} does not exist", source.display()))
        }
        Some(libc::ENOTDIR | libc::ELOOP) => {
            BoxliteError::InvalidArgument(format!("source {} is not a directory", source.display()))
        }
        _ => BoxliteError::Storage(format!("open source {}: {error}", source.display())),
    })
}

/// The target as the shim records it, so queries match what registration stored.
pub(crate) fn target_key(target: &Path) -> BoxliteResult<String> {
    normalize_target(&utf8(target, "mount target")?).map_err(|error| {
        BoxliteError::InvalidArgument(format!("invalid mount target {target:?}: {error}"))
    })
}

/// A request as the shim and the guest take it: an absolute UTF-8 source,
/// and the target as the shim records it.
#[derive(Debug)]
pub(crate) struct CheckedRequest {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) read_only: bool,
}

/// Validate a request. Invalid input stops here, before any descriptor or
/// record exists.
pub(crate) fn checked_request(request: &MountRequest) -> BoxliteResult<CheckedRequest> {
    if !request.source.is_absolute() {
        return Err(BoxliteError::InvalidArgument(format!(
            "source must be an absolute host path: {:?}",
            request.source
        )));
    }
    Ok(CheckedRequest {
        source: utf8(&request.source, "source")?,
        target: target_key(&request.target)?,
        read_only: request.read_only,
    })
}

fn utf8(path: &Path, what: &str) -> BoxliteResult<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        BoxliteError::InvalidArgument(format!("{what} must be valid UTF-8: {path:?}"))
    })
}

fn caller_state(state: RecordState) -> MountState {
    match state {
        RecordState::Preparing => MountState::Preparing,
        RecordState::Mounted => MountState::Mounted,
        RecordState::Detaching => MountState::Detaching,
    }
}

fn caller_info(record: &ControlMount, state: MountState) -> MountInfo {
    MountInfo {
        source: PathBuf::from(&record.source),
        target: PathBuf::from(&record.target),
        read_only: record.read_only,
        state,
    }
}

/// A failed shim exchange. Refusals keep their category; everything else is a
/// transport failure, whose outcome may be unknown.
fn shim_error(step: &str, error: io::Error) -> BoxliteError {
    let message = format!("{step}: {error}");
    match error.raw_os_error() {
        Some(libc::EBUSY) => BoxliteError::InvalidState(message),
        Some(libc::EINVAL) => BoxliteError::InvalidArgument(message),
        Some(libc::ENOSPC) => BoxliteError::ResourceExhausted(message),
        None => match error.kind() {
            io::ErrorKind::Unsupported => BoxliteError::Unsupported(message),
            io::ErrorKind::InvalidInput => BoxliteError::InvalidArgument(message),
            _ => BoxliteError::Storage(message),
        },
        _ => BoxliteError::Storage(message),
    }
}

fn guest_timeout_error(step: &str, target: &str, timeout: Duration) -> BoxliteError {
    BoxliteError::InvalidState(format!(
        "guest {step} of {target} did not finish within {timeout:?}"
    ))
}

/// The caller's error, plus what an unfinished removal leaves behind.
fn unconverged(error: BoxliteError, cleanup: BoxliteError) -> BoxliteError {
    match cleanup {
        stopped @ BoxliteError::Stopped(_) => stopped,
        cleanup => BoxliteError::InvalidState(format!(
            "{error}; removal is unconfirmed ({cleanup}), the record stays listed: retry remove"
        )),
    }
}

/// Whether the VM run a transaction was admitted to is still the live one.
fn ensure_current(is_current: &(dyn Fn() -> bool + Send + Sync)) -> BoxliteResult<()> {
    if is_current() {
        Ok(())
    } else {
        Err(BoxliteError::Stopped(
            "the box's VM run ended; use runtime.get() for its next run".into(),
        ))
    }
}

/// Runs one transaction in a task of its own: a caller that gives up cannot
/// abandon the cleanup, while the box's shutdown still ends it.
pub(super) async fn owned<T: Send + 'static>(
    shutdown: CancellationToken,
    transaction: impl Future<Output = BoxliteResult<T>> + Send + 'static,
) -> BoxliteResult<T> {
    let task = tokio::spawn(async move {
        tokio::select! {
            _ = shutdown.cancelled() => Err(BoxliteError::Stopped(
                "the box stopped during a mount operation".into(),
            )),
            result = transaction => result,
        }
    });
    match task.await {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err(BoxliteError::Internal(
            "mount operation was cancelled".into(),
        )),
        Err(error) => Err(BoxliteError::Internal(format!(
            "mount operation failed: {error}"
        ))),
    }
}

/// One operation's view of the admitted VM run.
pub(super) struct Transaction {
    pub(super) control: Arc<dyn MountControl>,
    pub(super) guest: Box<dyn GuestMounts>,
    pub(super) shutdown: CancellationToken,
    pub(super) is_current: Arc<dyn Fn() -> bool + Send + Sync>,
    /// How long a guest step may take before its outcome is unknown.
    pub(super) step_timeout: Duration,
}

impl Transaction {
    async fn open(run: MountRun) -> BoxliteResult<Self> {
        ensure_current(run.is_current.as_ref())?;
        let interface = run.guest.container().await?;
        Ok(Self {
            control: run.control,
            guest: Box::new(ContainerMounts {
                interface,
                container_id: run.container_id,
            }),
            shutdown: run.shutdown,
            is_current: run.is_current,
            step_timeout: GUEST_STEP_TIMEOUT,
        })
    }

    /// One blocking shim exchange, on a dedicated thread.
    pub(super) async fn control<T: Send + 'static>(
        &self,
        step: &'static str,
        exchange: impl FnOnce(&dyn MountControl) -> io::Result<T> + Send + 'static,
    ) -> BoxliteResult<T> {
        ensure_current(self.is_current.as_ref())?;
        let control = Arc::clone(&self.control);
        tokio::task::spawn_blocking(move || exchange(control.as_ref()))
            .await
            .map_err(|error| BoxliteError::Internal(format!("{step} task: {error}")))?
            .map_err(|error| shim_error(step, error))
    }

    pub(super) async fn add(
        mut self,
        request: CheckedRequest,
        directory: File,
    ) -> BoxliteResult<MountInfo> {
        let CheckedRequest {
            source: shim_source,
            target,
            read_only,
        } = request;
        let shim_target = target.clone();
        let record = match self
            .control("register the directory", move |control| {
                control.register(&shim_source, &shim_target, directory, read_only)
            })
            .await
        {
            Ok(record) => record,
            Err(BoxliteError::InvalidState(_)) => {
                return Err(BoxliteError::InvalidState(format!(
                    "a mount already exists at {target}"
                )));
            }
            // The reply can be lost after the shim published the record. Adopt
            // that record rather than fail an add that took effect.
            Err(error @ BoxliteError::Storage(_)) => self.preparing(&target).await?.ok_or(error)?,
            Err(error) => return Err(error),
        };
        ensure_current(self.is_current.as_ref())?;
        let mounted = tokio::time::timeout(
            self.step_timeout,
            self.guest.mount(&record.export, &target, read_only),
        )
        .await;
        let export = record.export.clone();
        let confirmed = match mounted {
            Ok(Ok(())) => {
                self.control("mark the mount mounted", move |control| {
                    control.set_state(&export, RecordState::Mounted)
                })
                .await
            }
            // The guest refused before touching the container, so nothing needs
            // detaching.
            Ok(Err(error @ BoxliteError::Unsupported(_))) => {
                self.mark_detaching(&record).await?;
                self.retire(&record).await?;
                return Err(error);
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(guest_timeout_error("mount", &target, self.step_timeout)),
        };
        match confirmed {
            Ok(()) => Ok(caller_info(&record, MountState::Mounted)),
            // The run is over; its records end with it.
            Err(error @ BoxliteError::Stopped(_)) => Err(error),
            Err(error) => match self.converge(&record).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(unconverged(error, cleanup)),
            },
        }
    }

    pub(super) async fn remove(mut self, target: &str) -> BoxliteResult<()> {
        let record = self
            .records()
            .await?
            .into_iter()
            .find(|record| record.target == target)
            .ok_or_else(|| BoxliteError::NotFound(format!("no runtime mount at {target}")))?;
        self.converge(&record).await
    }

    /// The shim's records, read on a fresh connection.
    async fn records(&self) -> BoxliteResult<Vec<ControlMount>> {
        self.control("list the records", |control| control.list())
            .await
    }

    /// The record a lost registration reply left behind, if any.
    async fn preparing(&self, target: &str) -> BoxliteResult<Option<ControlMount>> {
        Ok(self
            .records()
            .await?
            .into_iter()
            .find(|record| record.target == target && record.state == RecordState::Preparing))
    }

    /// Bring `record` to "no mount, no export": mark it `Detaching`, have the
    /// guest detach that identity, then retire the export. `Err` leaves the
    /// record listed, so a later remove can finish.
    async fn converge(&mut self, record: &ControlMount) -> BoxliteResult<()> {
        self.mark_detaching(record).await?;
        ensure_current(self.is_current.as_ref())?;
        let detached = tokio::time::timeout(
            self.step_timeout,
            self.guest.detach(&record.export, &record.target),
        )
        .await;
        match detached {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(guest_timeout_error(
                    "detach",
                    &record.target,
                    self.step_timeout,
                ));
            }
        }
        self.retire(record).await
    }

    async fn mark_detaching(&self, record: &ControlMount) -> BoxliteResult<()> {
        let export = record.export.clone();
        self.control("mark the mount detaching", move |control| {
            control.set_state(&export, RecordState::Detaching)
        })
        .await
    }

    /// Retire the export of a `Detaching` record. A record another caller
    /// already retired counts as done. `ENOENT` also names a missing socket,
    /// so the record list decides that, not the error alone.
    async fn retire(&self, record: &ControlMount) -> BoxliteResult<()> {
        let export = record.export.clone();
        let retired = {
            let export = export.clone();
            self.control("retire the export", move |control| control.remove(&export))
                .await
        };
        match retired {
            Ok(()) => Ok(()),
            Err(error) => match self.records().await {
                Ok(records) if !records.iter().any(|record| record.export == export) => Ok(()),
                _ => Err(error),
            },
        }
    }
}
