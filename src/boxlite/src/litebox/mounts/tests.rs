//! Coordinator and admission tests for runtime directory mounts.
//!
//! The shim and the guest are scripted here, so each outcome is forced. The
//! real planes meet `box.mounts()` in `tests/runtime_mounts.rs`.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use boxlite_shared::errors::{BoxliteError, BoxliteResult};

use super::transaction::{
    CheckedRequest, GuestMounts, MountControl, Transaction, checked_request, open_source, owned,
    target_key,
};
use super::{MountInfo, MountRequest, MountState};
use crate::BoxID;
use crate::experimental::ExperimentalFeatures;
use crate::litebox::box_impl::BoxImpl;
use crate::litebox::config::{BoxConfig, ContainerRuntimeConfig};
use crate::litebox::state::{BoxState, BoxStatus};
use crate::runtime::id::BoxIDMint;
use crate::runtime::options::{BoxOptions, BoxliteOptions, RootfsSpec};
use crate::runtime::rt_impl::{RuntimeImpl, SharedRuntimeImpl};
use crate::runtime::types::ContainerID;
use crate::vmm::VmmKind;
use crate::vmm::controller::mount_control::{ControlMount, MountState as RecordState};

const REGISTERED: [&str; 1] = ["register /workspace"];
const MOUNTED: [&str; 2] = ["register /workspace", "state mount-1 Mounted"];
const CONVERGED: [&str; 3] = [
    "register /workspace",
    "state mount-1 Detaching",
    "remove mount-1",
];
const RETIRING: [&str; 3] = ["list", "state mount-1 Detaching", "remove mount-1"];
/// A retirement whose error sends the coordinator back to the record list.
const CHECKED: [&str; 4] = ["list", "state mount-1 Detaching", "remove mount-1", "list"];

/// A stand-in for the shim records that logs every exchange.
#[derive(Default)]
struct ScriptedControl {
    records: Mutex<Vec<ControlMount>>,
    calls: Mutex<Vec<String>>,
    /// Publish the next registration, then fail its reply.
    lose_reply: AtomicBool,
    /// Fail the next `Mounted` mark with a transport error.
    lose_mounted_mark: AtomicBool,
    /// Fail the next retirement before it takes effect.
    lose_retire_reply: AtomicBool,
    /// How long every list blocks its thread.
    list_delay: Duration,
}

impl MountControl for ScriptedControl {
    fn register(
        &self,
        source: &str,
        target: &str,
        _directory: File,
        read_only: bool,
    ) -> io::Result<ControlMount> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("register {target}"));
        let mut records = self.records.lock().unwrap();
        if records.iter().any(|record| record.target == target) {
            return Err(io::Error::from_raw_os_error(libc::EBUSY));
        }
        let record = ControlMount {
            export: format!("mount-{}", records.len() + 1),
            source: source.to_owned(),
            target: target.to_owned(),
            read_only,
            state: RecordState::Preparing,
        };
        records.push(record.clone());
        drop(records);
        if self.lose_reply.swap(false, Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "registration reply lost",
            ));
        }
        Ok(record)
    }

    fn list(&self) -> io::Result<Vec<ControlMount>> {
        self.calls.lock().unwrap().push("list".into());
        std::thread::sleep(self.list_delay);
        Ok(self.records.lock().unwrap().clone())
    }

    fn set_state(&self, export: &str, state: RecordState) -> io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("state {export} {state:?}"));
        if state == RecordState::Mounted && self.lose_mounted_mark.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("shim unreachable"));
        }
        self.records
            .lock()
            .unwrap()
            .iter_mut()
            .find(|record| record.export == export)
            .map(|record| record.state = state)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))
    }

    fn remove(&self, export: &str) -> io::Result<()> {
        self.calls.lock().unwrap().push(format!("remove {export}"));
        let mut records = self.records.lock().unwrap();
        let index = records
            .iter()
            .position(|record| record.export == export)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        if records[index].state != RecordState::Detaching {
            return Err(io::Error::from_raw_os_error(libc::EBUSY));
        }
        if self.lose_retire_reply.swap(false, Ordering::SeqCst) {
            return Err(io::Error::other("shim unreachable"));
        }
        records.remove(index);
        Ok(())
    }
}

/// What the guest answers to one step.
#[derive(Clone, Copy, Default, PartialEq)]
enum Outcome {
    #[default]
    Ok,
    Refuse,
    Unsupported,
    /// Say nothing until the test releases it, then succeed.
    Hang,
}

/// A guest that answers on command, so a test controls each outcome.
#[derive(Clone)]
struct ScriptedGuest {
    calls: Arc<Mutex<Vec<String>>>,
    mount: Outcome,
    detach: Outcome,
    /// Runs when a detach arrives, before it is answered.
    on_detach: Option<Arc<dyn Fn() + Send + Sync>>,
    release: Arc<Semaphore>,
}

impl Default for ScriptedGuest {
    fn default() -> Self {
        Self {
            calls: Arc::default(),
            mount: Outcome::Ok,
            detach: Outcome::Ok,
            on_detach: None,
            release: Arc::new(Semaphore::new(0)),
        }
    }
}

impl ScriptedGuest {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    async fn answer(&self, outcome: Outcome) -> BoxliteResult<()> {
        match outcome {
            Outcome::Ok => Ok(()),
            Outcome::Refuse => Err(BoxliteError::Storage("injected guest refusal".into())),
            Outcome::Unsupported => Err(BoxliteError::Unsupported("CAP_SYS_ADMIN".into())),
            Outcome::Hang => {
                self.release
                    .acquire()
                    .await
                    .expect("release semaphore open")
                    .forget();
                Ok(())
            }
        }
    }
}

#[async_trait]
impl GuestMounts for ScriptedGuest {
    async fn mount(&mut self, export: &str, target: &str, read_only: bool) -> BoxliteResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("mount {export} {target} {read_only}"));
        self.answer(self.mount).await
    }

    async fn detach(&mut self, export: &str, target: &str) -> BoxliteResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("detach {export} {target}"));
        if let Some(on_detach) = &self.on_detach {
            on_detach();
        }
        self.answer(self.detach).await
    }
}

/// A transaction over scripted planes whose guest steps expire quickly.
fn transaction(control: Arc<ScriptedControl>, guest: ScriptedGuest) -> Transaction {
    Transaction {
        control,
        guest: Box::new(guest),
        shutdown: CancellationToken::new(),
        is_current: Arc::new(|| true),
        step_timeout: Duration::from_millis(40),
    }
}

/// Run `add` on a guest whose mount waits for `release`, the way the box does.
fn hanging_add(
    control: Arc<ScriptedControl>,
    shutdown: CancellationToken,
) -> (
    tokio::task::JoinHandle<BoxliteResult<MountInfo>>,
    Arc<Semaphore>,
) {
    let guest = ScriptedGuest {
        mount: Outcome::Hang,
        ..Default::default()
    };
    let release = Arc::clone(&guest.release);
    let caller = tokio::spawn(async move {
        owned(
            shutdown,
            transaction(control, guest).add(request("/workspace"), directory()),
        )
        .await
    });
    (caller, release)
}

fn request(target: &str) -> CheckedRequest {
    checked_request(&MountRequest {
        source: PathBuf::from("/srv/source"),
        target: PathBuf::from(target),
        read_only: false,
    })
    .expect("a valid request")
}

fn directory() -> File {
    File::open(std::env::temp_dir()).expect("open a directory")
}

fn calls(control: &ScriptedControl) -> Vec<String> {
    control.calls.lock().unwrap().clone()
}

fn records(control: &ScriptedControl) -> Vec<ControlMount> {
    control.records.lock().unwrap().clone()
}

/// Wait until `condition` holds, or fail the test.
async fn wait_until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition was not reached");
}

#[tokio::test]
async fn add_reports_mounted_only_after_the_guest_confirms() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest::default();

    let info = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect("add");

    assert_eq!(
        info,
        MountInfo {
            source: PathBuf::from("/srv/source"),
            target: PathBuf::from("/workspace"),
            read_only: false,
            state: MountState::Mounted,
        }
    );
    assert_eq!(calls(&control), MOUNTED);
    assert_eq!(guest.calls(), ["mount mount-1 /workspace false"]);
    assert_eq!(records(&control)[0].state, RecordState::Mounted);
}

#[tokio::test]
async fn a_guest_mount_failure_takes_the_removal_path_and_returns_the_guest_error() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest {
        mount: Outcome::Refuse,
        ..Default::default()
    };

    let error = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect_err("a refused mount");

    assert!(error.to_string().contains("injected"), "{error}");
    assert_eq!(calls(&control), CONVERGED);
    assert_eq!(
        guest.calls(),
        [
            "mount mount-1 /workspace false",
            "detach mount-1 /workspace"
        ]
    );
    assert!(records(&control).is_empty(), "the record is gone");
}

#[tokio::test]
async fn a_guest_mount_that_does_not_answer_is_an_unknown_outcome() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest {
        mount: Outcome::Hang,
        ..Default::default()
    };

    let error = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect_err("a mount that never answers");

    assert!(
        error.to_string().contains("did not finish within"),
        "{error}"
    );
    // The guest may have mounted without reporting it: the identity is
    // detached before the record goes away.
    assert_eq!(calls(&control), CONVERGED);
    assert_eq!(
        guest.calls(),
        [
            "mount mount-1 /workspace false",
            "detach mount-1 /workspace"
        ]
    );
    assert!(records(&control).is_empty());
}

#[tokio::test]
async fn a_capability_refusal_retires_the_record_without_detaching() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest {
        mount: Outcome::Unsupported,
        ..Default::default()
    };

    let error = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect_err("a refused container");

    assert!(matches!(error, BoxliteError::Unsupported(_)), "{error}");
    assert_eq!(calls(&control), CONVERGED);
    assert_eq!(guest.calls(), ["mount mount-1 /workspace false"]);
    assert!(records(&control).is_empty());
}

#[tokio::test]
async fn an_unrecorded_confirmation_removes_the_mount_the_guest_did_make() {
    let control = Arc::new(ScriptedControl::default());
    control.lose_mounted_mark.store(true, Ordering::SeqCst);
    let guest = ScriptedGuest::default();

    let error = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect_err("a mount the shim could not confirm");

    assert!(matches!(error, BoxliteError::Storage(_)), "{error}");
    assert_eq!(
        calls(&control),
        [
            "register /workspace",
            "state mount-1 Mounted",
            "state mount-1 Detaching",
            "remove mount-1",
        ]
    );
    assert_eq!(
        guest.calls(),
        [
            "mount mount-1 /workspace false",
            "detach mount-1 /workspace"
        ]
    );
    assert!(records(&control).is_empty());
}

#[tokio::test]
async fn a_lost_registration_reply_adopts_the_published_record() {
    let control = Arc::new(ScriptedControl::default());
    control.lose_reply.store(true, Ordering::SeqCst);
    let guest = ScriptedGuest::default();

    let info = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect("adopt the published record");

    assert_eq!(info.state, MountState::Mounted);
    assert_eq!(
        calls(&control),
        ["register /workspace", "list", "state mount-1 Mounted"]
    );
    assert_eq!(records(&control).len(), 1, "one record, not two");
}

#[tokio::test]
async fn an_occupied_target_is_reported_without_reaching_the_guest() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest::default();
    transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect("first add");

    let error = transaction(Arc::clone(&control), guest.clone())
        .add(request("/workspace"), directory())
        .await
        .expect_err("second add at the same target");

    assert!(
        matches!(&error, BoxliteError::InvalidState(message) if message.contains("already exists")),
        "{error}"
    );
    assert_eq!(guest.calls(), ["mount mount-1 /workspace false"]);
    assert_eq!(records(&control).len(), 1);
}

#[tokio::test]
async fn concurrent_adds_of_one_target_admit_exactly_one() {
    let control = Arc::new(ScriptedControl::default());
    let add = {
        let control = Arc::clone(&control);
        async move {
            transaction(control, ScriptedGuest::default())
                .add(request("/workspace"), directory())
                .await
        }
    };
    let other = {
        let control = Arc::clone(&control);
        async move {
            transaction(control, ScriptedGuest::default())
                .add(request("/workspace"), directory())
                .await
        }
    };

    let (winner, loser) = match tokio::join!(add, other) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        _ => panic!("exactly one add must succeed"),
    };
    assert_eq!(winner.state, MountState::Mounted);
    assert!(
        matches!(&loser, BoxliteError::InvalidState(message) if message.contains("already exists")),
        "{loser}"
    );
    assert_eq!(records(&control).len(), 1);
}

#[tokio::test]
async fn an_unconfirmed_removal_stays_listed_until_a_retry_finishes_it() {
    let control = Arc::new(ScriptedControl::default());
    let refusing = ScriptedGuest {
        mount: Outcome::Refuse,
        detach: Outcome::Refuse,
        ..Default::default()
    };

    let error = transaction(Arc::clone(&control), refusing)
        .add(request("/workspace"), directory())
        .await
        .expect_err("a mount whose cleanup fails");

    assert!(
        error.to_string().contains("removal is unconfirmed"),
        "{error}"
    );
    assert_eq!(records(&control)[0].state, RecordState::Detaching);

    transaction(Arc::clone(&control), ScriptedGuest::default())
        .remove("/workspace")
        .await
        .expect("the retry finishes the removal");

    assert!(records(&control).is_empty());
    assert_eq!(
        calls(&control),
        [
            "register /workspace",
            "state mount-1 Detaching",
            "list",
            "state mount-1 Detaching",
            "remove mount-1",
        ]
    );
}

#[tokio::test]
async fn an_interrupted_retirement_is_finished_by_the_next_removal() {
    let control = Arc::new(ScriptedControl::default());
    transaction(Arc::clone(&control), ScriptedGuest::default())
        .add(request("/workspace"), directory())
        .await
        .expect("add");
    control.lose_retire_reply.store(true, Ordering::SeqCst);
    let guest = ScriptedGuest::default();

    // The guest detached, then the retirement's reply was lost.
    let error = transaction(Arc::clone(&control), guest.clone())
        .remove("/workspace")
        .await
        .expect_err("an interrupted retirement");
    assert!(matches!(error, BoxliteError::Storage(_)), "{error}");
    assert_eq!(records(&control)[0].state, RecordState::Detaching);

    transaction(Arc::clone(&control), guest.clone())
        .remove("/workspace")
        .await
        .expect("the retry finishes the retirement");

    assert!(records(&control).is_empty());
    assert_eq!(
        calls(&control)[MOUNTED.len()..],
        [CHECKED.as_slice(), RETIRING.as_slice()].concat()
    );
    assert_eq!(
        guest.calls(),
        ["detach mount-1 /workspace", "detach mount-1 /workspace"]
    );
}

#[tokio::test]
async fn remove_detaches_by_identity_and_a_repeat_is_not_found() {
    let control = Arc::new(ScriptedControl::default());
    transaction(Arc::clone(&control), ScriptedGuest::default())
        .add(request("/workspace"), directory())
        .await
        .expect("add");
    let guest = ScriptedGuest::default();

    transaction(Arc::clone(&control), guest.clone())
        .remove("/workspace")
        .await
        .expect("remove");

    assert_eq!(guest.calls(), ["detach mount-1 /workspace"]);
    assert_eq!(
        calls(&control),
        [
            "register /workspace",
            "state mount-1 Mounted",
            "list",
            "state mount-1 Detaching",
            "remove mount-1",
        ]
    );
    let error = transaction(Arc::clone(&control), ScriptedGuest::default())
        .remove("/workspace")
        .await
        .expect_err("nothing is mounted");
    assert!(matches!(error, BoxliteError::NotFound(_)), "{error}");
}

#[tokio::test]
async fn a_stale_removal_leaves_a_replacement_at_the_target_alone() {
    let control = Arc::new(ScriptedControl::default());
    transaction(Arc::clone(&control), ScriptedGuest::default())
        .add(request("/workspace"), directory())
        .await
        .expect("add");
    // Another caller retires the mount and delivers a replacement at the
    // same target while this removal waits for the guest.
    let replacing: Arc<dyn Fn() + Send + Sync> = {
        let control = Arc::clone(&control);
        Arc::new(move || {
            let mut records = control.records.lock().unwrap();
            records.clear();
            records.push(ControlMount {
                export: "mount-2".into(),
                source: "/srv/other".into(),
                target: "/workspace".into(),
                read_only: true,
                state: RecordState::Mounted,
            });
        })
    };
    let guest = ScriptedGuest {
        on_detach: Some(replacing),
        ..Default::default()
    };

    transaction(Arc::clone(&control), guest.clone())
        .remove("/workspace")
        .await
        .expect("the stale removal converges");

    assert_eq!(guest.calls(), ["detach mount-1 /workspace"]);
    assert_eq!(calls(&control)[MOUNTED.len()..], CHECKED);
    let remaining = records(&control);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].export, "mount-2");
    assert_eq!(remaining[0].state, RecordState::Mounted);
}

#[tokio::test]
async fn a_caller_that_gives_up_does_not_abandon_the_transaction() {
    let control = Arc::new(ScriptedControl::default());
    let (caller, release) = hanging_add(Arc::clone(&control), CancellationToken::new());
    wait_until(|| calls(&control) == REGISTERED).await;

    caller.abort();
    assert!(caller.await.expect_err("aborted caller").is_cancelled());

    release.add_permits(1);
    let settled = || calls(&control) == MOUNTED;
    wait_until(settled).await;
}

#[tokio::test]
async fn shutdown_ends_an_in_flight_transaction_at_its_next_step() {
    let control = Arc::new(ScriptedControl::default());
    let shutdown = CancellationToken::new();
    let (caller, _release) = hanging_add(Arc::clone(&control), shutdown.clone());
    wait_until(|| calls(&control) == REGISTERED).await;

    shutdown.cancel();
    let error = caller.await.expect("caller ends").expect_err("stopped");

    assert!(matches!(error, BoxliteError::Stopped(_)), "{error}");
    // The run is over: nothing may reach the next run's records.
    assert_eq!(calls(&control), REGISTERED);
}

#[tokio::test]
async fn the_end_of_the_vm_run_stops_the_transaction_before_its_next_request() {
    let control = Arc::new(ScriptedControl::default());
    let guest = ScriptedGuest::default();
    let checks = Arc::new(AtomicUsize::new(0));
    let mut transaction = transaction(Arc::clone(&control), guest.clone());
    // The run ends right after registration: the check inside registration
    // passes, the one before the guest mount fails.
    let passed = Arc::clone(&checks);
    transaction.is_current = Arc::new(move || passed.fetch_add(1, Ordering::SeqCst) < 1);

    let error = transaction
        .add(request("/workspace"), directory())
        .await
        .expect_err("the run ended");

    assert!(matches!(error, BoxliteError::Stopped(_)), "{error}");
    assert!(guest.calls().is_empty(), "the guest is never asked");
    assert_eq!(calls(&control), REGISTERED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn blocking_exchanges_do_not_starve_a_single_executor_worker() {
    let control = Arc::new(ScriptedControl {
        list_delay: Duration::from_millis(200),
        ..Default::default()
    });
    let transaction = transaction(Arc::clone(&control), ScriptedGuest::default());
    let started = Instant::now();

    let (listed, ticker) = tokio::join!(
        transaction.control("list the records", |control| control.list()),
        async {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            started.elapsed()
        }
    );

    assert!(listed.expect("list").is_empty());
    assert!(
        ticker < Duration::from_millis(150),
        "other async work must progress while the shim exchange blocks: {ticker:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(200));
}

#[test]
fn invalid_requests_are_refused_before_any_exchange() {
    let non_utf8 = PathBuf::from(OsStr::from_bytes(b"/bad/\xff"));
    let valid = || MountRequest {
        source: PathBuf::from("/srv/source"),
        target: PathBuf::from("/workspace"),
        read_only: false,
    };
    let mut invalid = Vec::new();
    for target in [
        Path::new("/"),
        Path::new("relative"),
        Path::new("/a/../b"),
        non_utf8.as_path(),
    ] {
        invalid.push(MountRequest {
            target: target.to_path_buf(),
            ..valid()
        });
    }
    for source in [PathBuf::from("relative"), non_utf8] {
        invalid.push(MountRequest { source, ..valid() });
    }

    for request in invalid {
        let error = checked_request(&request).expect_err("an invalid request");
        assert!(matches!(error, BoxliteError::InvalidArgument(_)), "{error}");
    }

    let request = checked_request(&MountRequest {
        target: PathBuf::from("/workspace/"),
        ..valid()
    })
    .expect("a valid request");
    assert_eq!(request.target, "/workspace");
    assert_eq!(
        target_key(Path::new("/workspace/")).expect("a valid target"),
        "/workspace"
    );
    assert!(
        target_key(Path::new("/")).is_err(),
        "the root is not a target"
    );
}

#[tokio::test]
async fn opening_the_source_refuses_anything_but_a_directory() {
    let host = tempfile::tempdir_in("/tmp").expect("host directory");
    std::fs::write(host.path().join("file"), b"").expect("regular file");
    std::os::unix::fs::symlink(host.path(), host.path().join("link")).expect("symbolic link");

    for source in ["file", "link"] {
        let error = open_source(&host.path().join(source))
            .await
            .expect_err("refused source");
        assert!(error.to_string().contains("is not a directory"), "{error}");
    }
    let error = open_source(&host.path().join("absent"))
        .await
        .expect_err("absent source");
    assert!(error.to_string().contains("does not exist"), "{error}");

    open_source(host.path()).await.expect("a directory");
}

// Admission on the box -------------------------------------------------------

fn test_runtime(features: &str) -> (tempfile::TempDir, SharedRuntimeImpl) {
    let temp_dir = tempfile::tempdir_in("/tmp").expect("create temp dir");
    let runtime = RuntimeImpl::new_for_test_with_features(
        BoxliteOptions {
            home_dir: temp_dir.path().to_path_buf(),
            image_registries: vec![],
            ..Default::default()
        },
        ExperimentalFeatures::parse(features).expect("feature list"),
    )
    .expect("create runtime");
    (temp_dir, runtime)
}

/// A recorded box whose VM is not running, as `runtime.get()` returns it.
fn idle_box(runtime: &SharedRuntimeImpl, engine: VmmKind, status: BoxStatus) -> (BoxImpl, BoxID) {
    let id = BoxIDMint::mint();
    let config = BoxConfig {
        id: id.clone(),
        name: None,
        created_at: Utc::now(),
        container: ContainerRuntimeConfig {
            id: ContainerID::new(),
        },
        options: BoxOptions {
            rootfs: RootfsSpec::Image("alpine:latest".into()),
            ..Default::default()
        },
        engine_kind: engine,
        box_home: runtime.layout.boxes_dir().join(id.as_str()),
    };
    let mut state = BoxState::new();
    state.status = status;
    runtime
        .box_manager
        .add_box(&config, &state)
        .expect("persist test box");
    let box_impl = BoxImpl::new(
        config,
        state,
        Arc::clone(runtime),
        runtime.shutdown_token.child_token(),
    );
    (box_impl, id)
}

/// The box is still on record with no VM process, exactly as it was.
fn assert_still_idle(runtime: &SharedRuntimeImpl, id: &BoxID, status: BoxStatus) {
    let (_, persisted) = runtime
        .box_manager
        .box_by_id(id)
        .expect("load the box")
        .expect("the box remains registered");
    assert_eq!(persisted.status, status);
    assert_eq!(persisted.pid, None, "no VM was started");
}

fn add_request() -> MountRequest {
    MountRequest {
        source: PathBuf::from("/srv/source"),
        target: PathBuf::from("/workspace"),
        read_only: false,
    }
}

/// Run every mount operation on `box_impl` and collect their errors.
async fn mount_errors(box_impl: &BoxImpl) -> Vec<BoxliteError> {
    vec![
        box_impl
            .mounts_add(add_request())
            .await
            .expect_err("add is refused"),
        box_impl
            .mounts_remove(Path::new("/workspace"))
            .await
            .expect_err("remove is refused"),
        box_impl.mounts_list().await.expect_err("list is refused"),
        box_impl
            .mounts_get(Path::new("/workspace"))
            .await
            .expect_err("get is refused"),
    ]
}

#[tokio::test]
async fn every_mount_operation_needs_the_native_vmm_opt_in() {
    let (_temp_dir, runtime) = test_runtime("");
    let (box_impl, id) = idle_box(&runtime, VmmKind::Native, BoxStatus::Stopped);

    for error in mount_errors(&box_impl).await {
        assert!(matches!(error, BoxliteError::Config(_)), "{error}");
    }

    assert_still_idle(&runtime, &id, BoxStatus::Stopped);
}

#[tokio::test]
async fn every_mount_operation_needs_the_native_engine() {
    let (_temp_dir, runtime) = test_runtime("native-vmm");
    let (box_impl, id) = idle_box(&runtime, VmmKind::Libkrun, BoxStatus::Stopped);

    for error in mount_errors(&box_impl).await {
        assert!(matches!(error, BoxliteError::Unsupported(_)), "{error}");
    }

    assert_still_idle(&runtime, &id, BoxStatus::Stopped);
}

#[tokio::test]
async fn an_idle_box_answers_queries_empty_and_refuses_changes_without_starting() {
    let (_temp_dir, runtime) = test_runtime("native-vmm");

    for status in [BoxStatus::Configured, BoxStatus::Stopped] {
        let (box_impl, id) = idle_box(&runtime, VmmKind::Native, status);

        let error = box_impl
            .mounts_add(add_request())
            .await
            .expect_err("an idle box cannot take a mount");
        assert!(
            matches!(&error, BoxliteError::InvalidState(message) if message.contains("not running")),
            "{error}"
        );
        let error = box_impl
            .mounts_remove(Path::new("/workspace"))
            .await
            .expect_err("an idle box cannot lose a mount");
        assert!(matches!(error, BoxliteError::InvalidState(_)), "{error}");
        assert!(box_impl.mounts_list().await.expect("list").is_empty());
        assert!(
            box_impl
                .mounts_get(Path::new("/workspace"))
                .await
                .expect("get")
                .is_none()
        );

        assert_still_idle(&runtime, &id, status);
    }
}

#[tokio::test]
async fn an_invalid_request_is_refused_before_the_box_is_consulted() {
    let (_temp_dir, runtime) = test_runtime("native-vmm");
    let (box_impl, id) = idle_box(&runtime, VmmKind::Native, BoxStatus::Running);

    let error = box_impl
        .mounts_add(MountRequest {
            target: PathBuf::from("/a/../b"),
            ..add_request()
        })
        .await
        .expect_err("an unsafe target");
    assert!(matches!(error, BoxliteError::InvalidArgument(_)), "{error}");
    let error = box_impl
        .mounts_get(Path::new("/"))
        .await
        .expect_err("the root is not a mount target");
    assert!(matches!(error, BoxliteError::InvalidArgument(_)), "{error}");

    assert_still_idle(&runtime, &id, BoxStatus::Running);
}
