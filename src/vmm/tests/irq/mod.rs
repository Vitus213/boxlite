// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The interrupt sender's set/clear protocol and error propagation.

use std::{error::Error as _, io, sync::Mutex};

use super::*;

/// Records every `set_irq_line` call, optionally failing one.
struct RecordingTarget {
    calls: Mutex<Vec<(u32, bool)>>,
    fail_on: Option<(u32, bool)>,
}

impl RecordingTarget {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
        }
    }
}

impl InterruptTarget for RecordingTarget {
    fn set_irq_line(&self, line: u32, level: bool) -> Result<()> {
        if self.fail_on == Some((line, level)) {
            return Err(boxlite_hypervisor::Error::SetIrqLine {
                line,
                source: io::Error::other("injected"),
            });
        }
        self.calls.lock().unwrap().push((line, level));
        Ok(())
    }
}

/// Builds the fake both as a concrete handle (for assertions) and as a
/// trait object (for the sender).
fn recorded() -> (Arc<RecordingTarget>, Arc<dyn InterruptTarget>) {
    let target = Arc::new(RecordingTarget::new());
    let shared: Arc<dyn InterruptTarget> = target.clone();
    (target, shared)
}

fn failing(line: u32, level: bool) -> (Arc<RecordingTarget>, Arc<dyn InterruptTarget>) {
    let target = Arc::new(RecordingTarget {
        calls: Mutex::new(Vec::new()),
        fail_on: Some((line, level)),
    });
    let shared: Arc<dyn InterruptTarget> = target.clone();
    (target, shared)
}

fn calls(target: &RecordingTarget) -> Vec<(u32, bool)> {
    target.calls.lock().unwrap().clone()
}

#[test]
fn set_level_forwards_one_call_per_invocation() {
    let (target, shared) = recorded();
    let sender = IrqSender::new(shared);

    sender.set_level(4, true).unwrap();
    sender.set_level(4, false).unwrap();
    assert_eq!(calls(&target), vec![(4, true), (4, false)]);
}

#[test]
fn edge_is_a_set_followed_by_a_clear() {
    let (target, shared) = recorded();
    let sender = IrqSender::new(shared);

    sender.trigger_edge(5).unwrap();
    assert_eq!(calls(&target), vec![(5, true), (5, false)]);
}

#[test]
fn edge_reports_a_failed_clear_after_a_good_set() {
    let (target, shared) = failing(7, false);
    let sender = IrqSender::new(shared);

    let error = sender.trigger_edge(7).unwrap_err();
    // The set landed, the clear errored, and the host cause propagates.
    assert_eq!(calls(&target), vec![(7, true)]);
    assert_eq!(error.to_string(), "failed to set interrupt line 7");
    let cause = error
        .source()
        .and_then(|cause| cause.downcast_ref::<io::Error>())
        .expect("host cause");
    assert_eq!(cause.to_string(), "injected");
}

#[test]
fn set_failure_short_circuits_before_the_clear() {
    let (target, shared) = failing(9, true);
    let sender = IrqSender::new(shared);

    assert!(sender.trigger_edge(9).is_err());
    assert!(calls(&target).is_empty());
}

#[test]
fn sender_is_clonable_for_device_workers() {
    let (target, shared) = recorded();
    let sender = IrqSender::new(shared);

    sender.clone().set_level(4, true).unwrap();
    assert_eq!(calls(&target), vec![(4, true)]);
}

#[test]
fn set_level_propagates_a_failure() {
    let (target, shared) = failing(4, true);
    let sender = IrqSender::new(shared);

    let error = sender.set_level(4, true).unwrap_err();
    // No call landed: the target failed before recording.
    assert!(calls(&target).is_empty());
    assert_eq!(error.to_string(), "failed to set interrupt line 4");
    let cause = error
        .source()
        .and_then(|cause| cause.downcast_ref::<io::Error>())
        .expect("host cause");
    assert_eq!(cause.to_string(), "injected");
}

#[test]
fn a_backend_vm_is_an_interrupt_target() {
    // The blanket impl lets the lifecycle hand its backend VM to devices.
    let sender = IrqSender::new(Arc::new(crate::mock_vm::MockVm::new()));

    sender.set_level(4, true).unwrap();
    sender.trigger_edge(5).unwrap();
}
