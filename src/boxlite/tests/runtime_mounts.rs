//! Runtime directory mounts through `box.mounts()` on a real native Box.
//!
//! One container keeps running while host directories are delivered, queried
//! and lazily removed. No directory is shared at boot.
//!
//! Requires the native runtime, KVM and a kernel with virtio-fs:
//! `BOXLITE_TEST_ENGINE=native make test:integration:vmm:m2:run MATRIX_TEST_SUITES=runtime_mounts`.

#![cfg(all(feature = "native", target_os = "linux", target_arch = "x86_64"))]

mod common;

use boxlite::runtime::options::{BoxliteOptions, NetworkSpec};
use boxlite::{BoxCommand, BoxStatus, BoxliteError, LiteBox, MountRequest, MountState};
use common::home::PerTestBoxHome;
use futures::StreamExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

/// A host directory holding named files.
struct Source(tempfile::TempDir);

impl Source {
    fn new(name: &str, content: &str) -> Self {
        let dir = tempfile::tempdir_in("/tmp").expect("host source directory");
        std::fs::write(dir.path().join(name), content).expect("seed host source");
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.path().to_path_buf()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.path().join(name)).expect("read host source")
    }

    /// Add a file the host user cannot write.
    fn keep_read_only(&self, name: &str, content: &str) {
        let path = self.0.path().join(name);
        std::fs::write(&path, content).expect("seed read-only file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444))
            .expect("make the file read-only");
    }
}

fn request(source: &Source, target: &str, read_only: bool) -> MountRequest {
    MountRequest {
        source: source.path(),
        target: PathBuf::from(target),
        read_only,
    }
}

fn box_options() -> boxlite::runtime::options::BoxOptions {
    let mut options = common::alpine_opts();
    options.network = NetworkSpec::Disabled;
    options.cmd = Some(vec!["sleep".into(), "300".into()]);
    assert!(options.volumes.is_empty(), "nothing is shared at boot");
    options
}

fn runtime_options(home: &PerTestBoxHome) -> BoxliteOptions {
    BoxliteOptions {
        home_dir: home.path.clone(),
        image_registries: common::test_registries(),
        ..Default::default()
    }
}

async fn running_box(home: &PerTestBoxHome) -> (common::TestRuntime, LiteBox) {
    let runtime = common::TestRuntime::new(runtime_options(home)).expect("create runtime");
    let bx = runtime
        .create(box_options(), None)
        .await
        .expect("create box");
    bx.start().await.expect("start box");
    (runtime, bx)
}

/// Run `script` in the container and return its exit code and stdout.
async fn run(bx: &LiteBox, script: &str) -> (i32, String) {
    let mut execution = bx
        .exec(BoxCommand::new("sh").args(["-c", script]))
        .await
        .expect("start command");
    let mut output = String::new();
    if let Some(mut stream) = execution.stdout() {
        while let Some(chunk) = stream.next().await {
            output.push_str(&chunk);
        }
    }
    (
        execution.wait().await.expect("wait for command").exit_code,
        output,
    )
}

/// Poll until `probe` exits 0, or fail.
async fn wait_until(bx: &LiteBox, probe: &str) {
    for _ in 0..100 {
        if run(bx, probe).await.0 == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("`{probe}` did not become true");
}

/// Poll until `target` holds the given state, or fail.
async fn wait_for_state(mounts: &boxlite::MountsHandle, target: &str, state: MountState) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if mounts
                .get(target)
                .await
                .expect("get")
                .is_some_and(|mount| mount.state == state)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the mount did not reach the state");
}

/// Puts a hidden control socket back when dropped, so a failed assertion
/// cannot leave the box's shim unreachable for the stop that follows.
struct RestoreSocket {
    hidden: PathBuf,
    path: PathBuf,
}

impl Drop for RestoreSocket {
    fn drop(&mut self) {
        let _ = std::fs::rename(&self.hidden, &self.path);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn mounts_are_delivered_queried_and_lazily_removed_while_the_container_runs() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let (runtime, bx) = running_box(&home).await;
    let mounts = bx.mounts();
    assert!(mounts.list().await.expect("list").is_empty());
    assert!(mounts.get("/mnt/late").await.expect("get").is_none());

    // Add on the single executor worker while a command runs beside it.
    let source = Source::new("value", "host");
    let (added, command) = tokio::join!(
        mounts.add(request(&source, "/mnt/late", false)),
        run(&bx, "echo alive")
    );
    let info = added.expect("runtime add");
    assert_eq!(command, (0, "alive\n".into()));
    assert_eq!(info.state, MountState::Mounted);
    assert_eq!(
        run(&bx, "cat /mnt/late/value").await,
        (0, "host".into()),
        "the container reads the delivered directory"
    );

    // Query: the record reflects the mount; nothing else is mounted.
    assert_eq!(
        mounts.get("/mnt/late").await.expect("get"),
        Some(info.clone())
    );
    assert_eq!(mounts.list().await.expect("list"), vec![info.clone()]);
    assert_eq!(mounts.get("/mnt/absent").await.expect("get"), None);
    assert_eq!(info.source, source.path());
    assert!(!info.read_only);

    // A writable mount writes through to the host directory.
    assert_eq!(run(&bx, "printf guest > /mnt/late/from-guest").await.0, 0);
    assert_eq!(source.read("from-guest"), "guest");

    // Host permissions still apply to a writable mount.
    source.keep_read_only("locked", "locked");
    assert_ne!(run(&bx, "printf bad > /mnt/late/locked").await.0, 0);
    assert_eq!(source.read("locked"), "locked");

    // A read-only mount refuses changes and leaves the host directory as is.
    let kept = Source::new("keep", "read-only");
    let read_only = mounts
        .add(request(&kept, "/mnt/late-ro", true))
        .await
        .expect("read-only add");
    assert!(read_only.read_only);
    assert_eq!(
        mounts.get("/mnt/late-ro").await.expect("get"),
        Some(read_only.clone())
    );
    assert_ne!(run(&bx, "touch /mnt/late-ro/refused").await.0, 0);
    assert_eq!(kept.read("keep"), "read-only");

    // An existing empty directory is accepted as a target.
    assert_eq!(run(&bx, "mkdir /mnt/empty").await.0, 0);
    mounts
        .add(request(&source, "/mnt/empty", false))
        .await
        .expect("add onto an empty directory");
    assert_eq!(run(&bx, "cat /mnt/empty/value").await.1, "host");
    mounts.remove("/mnt/empty").await.expect("remove");

    // Target rules: an occupied target is refused and keeps its content.
    assert_eq!(
        run(&bx, "mkdir -p /mnt/full && printf x > /mnt/full/occupied")
            .await
            .0,
        0
    );
    let error = mounts
        .add(request(&source, "/mnt/full", false))
        .await
        .expect_err("a non-empty target is refused");
    assert!(
        matches!(error, BoxliteError::Rpc(_)),
        "the guest refuses the target: {error}"
    );
    assert_eq!(run(&bx, "cat /mnt/full/occupied").await.1, "x");
    assert!(mounts.get("/mnt/full").await.expect("get").is_none());
    // The refused add left no reservation behind.
    assert_eq!(run(&bx, "rm /mnt/full/occupied").await.0, 0);
    mounts
        .add(request(&source, "/mnt/full", false))
        .await
        .expect("the target is free once it is empty");
    mounts.remove("/mnt/full").await.expect("remove");

    // A target reached through a symbolic link is refused and stays a link.
    assert_eq!(
        run(
            &bx,
            "mkdir -p /tmp/behind && ln -s /tmp/behind /mnt/link && test -L /mnt/link"
        )
        .await
        .0,
        0
    );
    assert!(
        mounts
            .add(request(&source, "/mnt/link", false))
            .await
            .is_err(),
        "the guest refuses a target reached through a link"
    );
    assert_ne!(run(&bx, "test -e /tmp/behind/value").await.0, 0);
    assert!(mounts.get("/mnt/link").await.expect("get").is_none());

    // Target rules: the root, relative paths and `..` are refused by the host.
    for target in ["/", "relative", "/mnt/../late"] {
        let error = mounts
            .add(MountRequest {
                target: PathBuf::from(target),
                ..request(&source, target, false)
            })
            .await
            .expect_err("an unsafe target");
        assert!(
            matches!(error, BoxliteError::InvalidArgument(_)),
            "{target}: {error}"
        );
    }
    let error = mounts
        .add(request(&source, "/mnt/late", false))
        .await
        .expect_err("an occupied target");
    assert!(matches!(error, BoxliteError::InvalidState(_)), "{error}");

    // Lazy removal: a process holding a file and a directory descriptor
    // neither blocks the removal nor loses what it opened.
    let holder = bx
        .exec(BoxCommand::new("sh").args([
            "-c",
            concat!(
                "exec 3</mnt/late/value; exec 4</mnt/late; touch /tmp/held-ready; ",
                "while [ ! -e /tmp/read-held ]; do sleep 0.05; done; ",
                "cat <&3 > /tmp/held-file; cat /proc/self/fd/4/value > /tmp/held-dir"
            ),
        ]))
        .await
        .expect("start holder");
    wait_until(&bx, "test -e /tmp/held-ready").await;
    mounts.remove("/mnt/late").await.expect("lazy removal");
    assert_eq!(run(&bx, "test ! -e /mnt/late/value").await.0, 0);
    assert!(mounts.get("/mnt/late").await.expect("get").is_none());
    assert_eq!(mounts.list().await.expect("list"), vec![read_only.clone()]);
    assert!(matches!(
        mounts.remove("/mnt/late").await,
        Err(BoxliteError::NotFound(_))
    ));

    // Same path, another source: the replacement is independent.
    let replacement = Source::new("value", "replacement");
    let again = mounts
        .add(request(&replacement, "/mnt/late", true))
        .await
        .expect("remount the same target");
    assert_eq!(again.target, PathBuf::from("/mnt/late"));
    assert_ne!(again.read_only, info.read_only);
    assert_eq!(
        run(&bx, "cat /mnt/late/value").await,
        (0, "replacement".into())
    );
    assert_ne!(run(&bx, "printf bad > /mnt/late/value").await.0, 0);
    assert_eq!(replacement.read("value"), "replacement");

    // The holder still reads the directory it opened before the removal.
    assert_eq!(run(&bx, "touch /tmp/read-held").await.0, 0);
    assert_eq!(holder.wait().await.expect("holder exits").exit_code, 0);
    assert_eq!(
        run(&bx, "cat /tmp/held-file /tmp/held-dir").await,
        (0, "hosthost".into())
    );

    for target in ["/mnt/late", "/mnt/late-ro"] {
        mounts.remove(target).await.expect("remove");
    }
    assert!(mounts.list().await.expect("list").is_empty());

    // After a stop the records are gone, the box is not started again, and
    // a change is refused.
    bx.stop().await.expect("stop box");
    assert!(bx.mounts().list().await.expect("list").is_empty());
    assert!(bx.mounts().get("/mnt/late").await.expect("get").is_none());
    assert!(matches!(
        bx.mounts().add(request(&source, "/mnt/late", false)).await,
        Err(BoxliteError::InvalidState(_))
    ));
    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}

#[tokio::test]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn two_clients_adding_one_target_admit_a_single_mount() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let (runtime, bx) = running_box(&home).await;
    // One runtime owns a home; its clients are separate handles of the same box.
    let other_box = runtime
        .get(bx.id().as_str())
        .await
        .expect("look up the box")
        .expect("the box is recorded");
    let source = Source::new("value", "raced");

    let (mounts, other_mounts) = (bx.mounts(), other_box.mounts());
    let (first, second) = tokio::join!(
        mounts.add(request(&source, "/mnt/raced", false)),
        other_mounts.add(request(&source, "/mnt/raced", true)),
    );

    let (winner, loser) = match (first, second) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        _ => panic!("exactly one add must succeed"),
    };
    assert_eq!(winner.state, MountState::Mounted);
    assert!(
        matches!(&loser, BoxliteError::InvalidState(message) if message.contains("already exists")),
        "{loser}"
    );
    // The winner's mount and record are complete, and every client sees them.
    assert_eq!(
        bx.mounts().list().await.expect("list"),
        vec![winner.clone()]
    );
    assert_eq!(
        other_box.mounts().get("/mnt/raced").await.expect("get"),
        Some(winner.clone())
    );
    assert_eq!(run(&bx, "cat /mnt/raced/value").await, (0, "raced".into()));
    other_box
        .mounts()
        .remove("/mnt/raced")
        .await
        .expect("remove");
    assert!(bx.mounts().list().await.expect("list").is_empty());

    bx.stop().await.expect("stop box");
    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}

#[tokio::test]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn records_end_with_the_vm_run_and_an_old_handle_cannot_touch_the_next() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let (runtime, bx) = running_box(&home).await;
    let old = bx.mounts();
    let source = Source::new("value", "first run");
    old.add(request(&source, "/mnt/run", false))
        .await
        .expect("mount in the first run");
    bx.stop().await.expect("stop box");

    // The old run is over: nothing is left for its handle to act on.
    assert!(old.list().await.expect("list").is_empty());
    assert!(matches!(
        old.remove("/mnt/run").await,
        Err(BoxliteError::InvalidState(_))
    ));
    assert!(matches!(
        old.add(request(&source, "/mnt/run", false)).await,
        Err(BoxliteError::InvalidState(_))
    ));

    // The next run starts with no records.
    let next = runtime
        .get(bx.id().as_str())
        .await
        .expect("look up the box")
        .expect("the box is recorded");
    next.start().await.expect("start again");
    assert!(next.mounts().list().await.expect("list").is_empty());
    assert!(next.mounts().get("/mnt/run").await.expect("get").is_none());

    // A stale removal from the old handle cannot reach the new run's mount.
    let replacement = Source::new("value", "second run");
    let mounted = next
        .mounts()
        .add(request(&replacement, "/mnt/run", false))
        .await
        .expect("mount in the next run");
    assert!(matches!(
        old.remove("/mnt/run").await,
        Err(BoxliteError::InvalidState(_))
    ));
    assert_eq!(
        next.mounts().get("/mnt/run").await.expect("get"),
        Some(mounted)
    );
    assert_eq!(
        run(&next, "cat /mnt/run/value").await,
        (0, "second run".into())
    );

    next.stop().await.expect("stop box");
    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}

#[tokio::test]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn a_caller_that_gives_up_leaves_the_transaction_to_finish() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let (runtime, bx) = running_box(&home).await;
    let mounts = bx.mounts();
    let source = Source::new("value", "late");

    // The caller leaves as soon as the transaction has registered its record,
    // before the real guest has answered. If the guest answered first, the add
    // simply completed.
    let registered = async {
        while mounts.get("/mnt/late").await.expect("get").is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::pin!(registered);
    tokio::select! {
        added = mounts.add(request(&source, "/mnt/late", false)) => {
            assert_eq!(added.expect("add").state, MountState::Mounted);
        }
        () = &mut registered => {}
    }

    // The transaction keeps its owner: the real guest confirms it, and the
    // record is complete without the caller.
    wait_for_state(&mounts, "/mnt/late", MountState::Mounted).await;
    assert_eq!(run(&bx, "cat /mnt/late/value").await, (0, "late".into()));
    mounts.remove("/mnt/late").await.expect("remove");
    assert!(mounts.list().await.expect("list").is_empty());

    bx.stop().await.expect("stop box");
    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}

#[tokio::test]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn an_unreachable_shim_leaves_the_record_for_a_retry() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let (runtime, bx) = running_box(&home).await;
    let mounts = bx.mounts();
    let source = Source::new("value", "kept");
    let info = mounts
        .add(request(&source, "/mnt/kept", false))
        .await
        .expect("add");
    // Hide the shim's control socket, so the shim cannot be reached. A failed
    // assertion puts it back.
    let sockets = home
        .path
        .join("boxes")
        .join(bx.id().as_str())
        .join("sockets");
    let control_socket =
        boxlite::net::socket_path::BoxSockets::new(bx.id().to_string(), &sockets).vmm_sock();
    let _restore = RestoreSocket {
        hidden: control_socket.with_extension("unreachable"),
        path: control_socket.clone(),
    };
    std::fs::rename(&control_socket, &_restore.hidden).expect("hide the control socket");

    // The shim cannot answer, so the removal fails and keeps the record.
    let error = mounts
        .remove("/mnt/kept")
        .await
        .expect_err("the shim cannot answer");
    assert!(matches!(error, BoxliteError::Storage(_)), "{error}");
    std::fs::rename(&_restore.hidden, &control_socket).expect("restore the control socket");
    assert_eq!(mounts.get("/mnt/kept").await.expect("get"), Some(info));
    mounts
        .remove("/mnt/kept")
        .await
        .expect("the retry removes the mount");
    assert!(mounts.list().await.expect("list").is_empty());
    assert_eq!(run(&bx, "test ! -e /mnt/kept/value").await.0, 0);

    bx.stop().await.expect("stop box");
    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}

#[tokio::test]
#[ignore = "requires native runtime, KVM and VMM_KERNEL; MATRIX_TEST_SUITES=runtime_mounts"]
async fn an_idle_box_answers_queries_empty_and_refuses_changes() {
    assert_eq!(
        std::env::var("BOXLITE_TEST_ENGINE").as_deref(),
        Ok("native")
    );
    let home = PerTestBoxHome::new();
    let runtime = common::TestRuntime::new(runtime_options(&home)).expect("create runtime");
    let bx = runtime
        .create(box_options(), None)
        .await
        .expect("create box");
    let source = Source::new("value", "idle");

    assert!(bx.mounts().list().await.expect("list").is_empty());
    assert!(bx.mounts().get("/mnt/idle").await.expect("get").is_none());
    assert!(matches!(
        bx.mounts().add(request(&source, "/mnt/idle", false)).await,
        Err(BoxliteError::InvalidState(_))
    ));
    assert_eq!(bx.info().await.expect("info").status, BoxStatus::Configured);

    runtime
        .remove(bx.id().as_str(), true)
        .await
        .expect("remove box");
}
