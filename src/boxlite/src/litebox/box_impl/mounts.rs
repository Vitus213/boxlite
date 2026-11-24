//! Runtime directory mounts of a box: admission, then the transaction.
//!
//! Every operation first requires the `native-vmm` experimental opt-in and
//! the native engine, so an unsupported box is refused before anything is
//! opened or registered. Changes then need the running VM; queries answer from
//! the shim's records while the box runs and are empty otherwise. Nothing here
//! starts or restarts the box.

use std::path::Path;
use std::sync::Arc;

use boxlite_shared::errors::{BoxliteError, BoxliteResult};

use super::BoxImpl;
use crate::experimental::ExperimentalFeature;
use crate::litebox::mounts::transaction::{self, MountRun};
use crate::litebox::mounts::{MountInfo, MountRequest};
use crate::runtime::types::BoxStatus;
use crate::vmm::VmmKind;
use crate::vmm::controller::mount_control::MountControlClient;

impl BoxImpl {
    /// Admission shared by every runtime mount operation: the `native-vmm`
    /// experimental opt-in and the native engine. Neither starts the box.
    fn require_runtime_mounts(&self) -> BoxliteResult<()> {
        self.runtime
            .experimental_features
            .require(ExperimentalFeature::NativeVmm)?;
        if self.config.engine_kind != VmmKind::Native {
            return Err(BoxliteError::Unsupported(format!(
                "runtime directory mounts need the native VMM; box {} runs {:?}",
                self.id(),
                self.config.engine_kind
            )));
        }
        Ok(())
    }

    /// Deliver a host directory into the running container.
    pub(crate) async fn mounts_add(&self, request: MountRequest) -> BoxliteResult<MountInfo> {
        self.require_runtime_mounts()?;
        let request = transaction::checked_request(&request)?;
        let run = self.mount_run().await?;
        let directory = transaction::open_source(Path::new(&request.source)).await?;
        transaction::add(run, request, directory).await
    }

    /// Lazily remove the runtime mount at `target`.
    pub(crate) async fn mounts_remove(&self, target: &Path) -> BoxliteResult<()> {
        self.require_runtime_mounts()?;
        let target = transaction::target_key(target)?;
        let run = self.mount_run().await?;
        transaction::remove(run, &target).await
    }

    /// Every runtime mount of the box's current VM run, empty while no VM runs.
    pub(crate) async fn mounts_list(&self) -> BoxliteResult<Vec<MountInfo>> {
        self.require_runtime_mounts()?;
        match self.mount_records().await? {
            Some(control) => transaction::list(control).await,
            None => Ok(Vec::new()),
        }
    }

    /// The runtime mount at `target`, or `None` while nothing is mounted there.
    pub(crate) async fn mounts_get(&self, target: &Path) -> BoxliteResult<Option<MountInfo>> {
        let target = transaction::target_key(target)?;
        Ok(self
            .mounts_list()
            .await?
            .into_iter()
            .find(|mount| mount.target == Path::new(&target)))
    }

    /// The shim's records while the box runs, or `None` while no VM can hold
    /// mounts. Adopting a running VM never starts one.
    async fn mount_records(&self) -> BoxliteResult<Option<Arc<MountControlClient>>> {
        let status = self.state.read().status;
        if self.shutdown_token.is_cancelled() || status != BoxStatus::Running {
            return match status {
                // A paused VM still holds its mounts; reporting none would be wrong.
                BoxStatus::Paused => Err(BoxliteError::InvalidState(format!(
                    "Box {} is paused; its runtime mounts are unavailable until it resumes",
                    self.config.id
                ))),
                _ => Ok(None),
            };
        }
        self.existing_live_state().await?;
        Ok(Some(self.mount_control()))
    }

    /// The running VM's view for one mount transaction. A box that is not
    /// running is reported, never started. Like `exec`, a change waits for the
    /// container's start, which `start()` leaves running in the background.
    async fn mount_run(&self) -> BoxliteResult<MountRun> {
        let live = self.existing_live_state().await?;
        self.ensure_container_started(live).await?;
        let state = Arc::clone(&self.state);
        let shutdown = self.shutdown_token.clone();
        let watch = shutdown.clone();
        let (pid, started_at) = {
            let state = state.read();
            (state.pid, state.started_at)
        };
        Ok(MountRun {
            control: self.mount_control(),
            guest: live.guest_session.clone(),
            container_id: self.container_id().to_owned(),
            is_current: Arc::new(move || {
                let state = state.read();
                !watch.is_cancelled()
                    && state.status == BoxStatus::Running
                    && state.pid == pid
                    && state.started_at == started_at
            }),
            shutdown,
        })
    }

    fn mount_control(&self) -> Arc<MountControlClient> {
        Arc::new(MountControlClient::new(self.layout.sockets().vmm_sock()))
    }
}
