//! Runtime directory mounts on a running box: add, remove, list and get.
//!
//! A runtime mount spans two planes: the shim's records on `vmm.sock` and the
//! guest's mount table over `box.sock`. The shim owns the record and reserves
//! the target; the guest confirms what the container sees. [`transaction`]
//! sequences the two and converges every failure onto the removal path.
//!
//! Runtime mounts need a running Linux x86_64 native box under the
//! `native-vmm` experimental opt-in. Records belong to one VM run and do not
//! survive a stop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use boxlite_shared::errors::BoxliteResult;

use crate::runtime::backend::BoxBackend;

#[cfg(all(test, feature = "native", target_os = "linux", target_arch = "x86_64"))]
mod tests;
#[cfg(all(feature = "native", target_os = "linux", target_arch = "x86_64"))]
pub(crate) mod transaction;

/// A host directory to deliver into the running container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRequest {
    /// Host directory to deliver. A symbolic link is refused at open.
    pub source: PathBuf,
    /// Absolute container path below `/`, without `..`. Missing directories
    /// are created; a path that has entries, or one reached through a symbolic
    /// link, is refused.
    pub target: PathBuf,
    /// Whether the container sees the directory read-only.
    pub read_only: bool,
}

/// Where a runtime mount is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MountState {
    /// Registered with the shim; the guest has not confirmed the mount yet.
    Preparing,
    /// Visible in the container at the target path.
    Mounted,
    /// Removal is incomplete; repeating the removal finishes it.
    Detaching,
}

/// A runtime mount as callers see it. The shim's export identity stays internal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountInfo {
    /// Host directory the content comes from.
    pub source: PathBuf,
    /// Container path the directory is visible at.
    pub target: PathBuf,
    /// Whether the container sees the directory read-only.
    pub read_only: bool,
    /// Where the mount is in its lifecycle.
    pub state: MountState,
}

/// Runtime directory mounts of a box. Reached through
/// [`LiteBox::mounts`](super::LiteBox::mounts).
#[derive(Clone)]
pub struct MountsHandle {
    backend: Arc<dyn BoxBackend>,
}

impl MountsHandle {
    /// Retain the backend without starting it; the backend decides support.
    pub(super) fn new(backend: Arc<dyn BoxBackend>) -> Self {
        Self { backend }
    }

    /// Deliver a host directory into the running container.
    ///
    /// Succeeds only after the guest confirmed the mount. A stopped box is
    /// reported, never started. A target that already has a mount, or that is
    /// not an empty container path, is refused.
    pub async fn add(&self, request: MountRequest) -> BoxliteResult<MountInfo> {
        self.backend.mounts_add(request).await
    }

    /// Lazily remove the mount at `target`: the path leaves the container at
    /// once while descriptors opened before keep working. Succeeds only after
    /// the guest confirmed the detach. Returns `NotFound` when no mount is at
    /// `target`.
    pub async fn remove(&self, target: impl AsRef<Path>) -> BoxliteResult<()> {
        self.backend.mounts_remove(target.as_ref()).await
    }

    /// Every mount of the current VM run.
    pub async fn list(&self) -> BoxliteResult<Vec<MountInfo>> {
        self.backend.mounts_list().await
    }

    /// The mount at `target`, or `None` when no mount is there.
    pub async fn get(&self, target: impl AsRef<Path>) -> BoxliteResult<Option<MountInfo>> {
        self.backend.mounts_get(target.as_ref()).await
    }
}
