// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

//! The console queue between the serial port and the host sink.
//!
//! A vCPU serves a serial write under the device lock, so [`ConsoleWriter`]
//! pushes the byte into a bounded queue and returns; a worker thread writes
//! the queue to the sink. A stalled or failing sink never stalls the guest:
//! a full queue drops its oldest bytes, as the kernel's log ring does, and
//! counts them, like a batch the sink refuses and a write after `finish`.

use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Bytes the queue holds before it drops the oldest.
pub(crate) const DEFAULT_CAPACITY: usize = 64 * 1024;

/// How long `finish` waits: enough for a healthy sink to take a full queue.
const FINISH_TIMEOUT: Duration = Duration::from_secs(1);

struct Queue {
    bytes: VecDeque<u8>,
    /// Bytes the worker took and is writing.
    in_flight: usize,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    /// Signals the worker that bytes arrived or the queue closed.
    ready: Condvar,
    capacity: usize,
    dropped: AtomicUsize,
}

impl Shared {
    /// Locks the queue, which a panic under the lock leaves consistent.
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Closes the queue: the worker writes what is left and ends.
    fn close(&self) {
        self.queue().closed = true;
        self.ready.notify_one();
    }
}

/// The serial port's sink: a write queues the bytes and returns.
pub(crate) struct ConsoleWriter(Arc<Shared>);

impl Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut queue = self.0.queue();
        if queue.closed {
            self.0.dropped.fetch_add(buf.len(), Ordering::Relaxed);
            return Ok(buf.len());
        }
        for &byte in buf {
            if queue.bytes.len() == self.0.capacity {
                queue.bytes.pop_front();
                self.0.dropped.fetch_add(1, Ordering::Relaxed);
            }
            queue.bytes.push_back(byte);
        }
        drop(queue);
        self.0.ready.notify_one();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The worker thread that drains the queue into the sink.
pub(crate) struct ConsoleWorker {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
    /// Disconnects when the worker ends, panics included.
    ended: mpsc::Receiver<()>,
}

impl ConsoleWorker {
    /// Starts a worker writing to `sink`, with a queue of `capacity` bytes,
    /// and returns the writer the serial port uses; refuses a capacity of 0.
    pub(crate) fn spawn(
        sink: Box<dyn Write + Send>,
        capacity: usize,
    ) -> io::Result<(ConsoleWriter, Self)> {
        if capacity == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                bytes: VecDeque::with_capacity(capacity),
                in_flight: 0,
                closed: false,
            }),
            ready: Condvar::new(),
            capacity,
            dropped: AtomicUsize::new(0),
        });
        let (ended_sender, ended) = mpsc::channel::<()>();
        let thread = thread::Builder::new().name("console".into()).spawn({
            let shared = Arc::clone(&shared);
            move || {
                let _ended = ended_sender;
                drain(&shared, sink);
            }
        })?;
        Ok((
            ConsoleWriter(Arc::clone(&shared)),
            Self {
                shared,
                thread: Some(thread),
                ended,
            },
        ))
    }

    /// Bytes dropped: the queue was full or closed, or the sink never took them.
    pub(crate) fn dropped_bytes(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed) as u64
    }

    /// Closes the queue and waits up to [`FINISH_TIMEOUT`] for the worker to
    /// write the rest. A sink write cannot be cancelled, so then the worker
    /// is left behind and every unwritten byte, its held batch included,
    /// counts as dropped. A second call does nothing.
    pub(crate) fn finish(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.shared.close();
        if let Err(mpsc::RecvTimeoutError::Disconnected) = self.ended.recv_timeout(FINISH_TIMEOUT) {
            let _ = thread.join();
            return;
        }
        let mut queue = self.shared.queue();
        let unwritten = queue.bytes.drain(..).count() + std::mem::take(&mut queue.in_flight);
        self.shared.dropped.fetch_add(unwritten, Ordering::Relaxed);
    }
}

impl Drop for ConsoleWorker {
    fn drop(&mut self) {
        self.shared.close(); // Without `finish`, nothing waits for the worker.
    }
}

/// The worker loop: takes everything queued, writes it, and ends once the
/// queue is closed and empty.
fn drain(shared: &Shared, mut sink: Box<dyn Write + Send>) {
    loop {
        let mut queue = shared.queue();
        while queue.bytes.is_empty() && !queue.closed {
            queue = shared
                .ready
                .wait(queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if queue.bytes.is_empty() {
            return;
        }
        let pending: Vec<u8> = queue.bytes.drain(..).collect();
        queue.in_flight = pending.len();
        drop(queue);
        let written = sink.write_all(&pending).and_then(|()| sink.flush());
        let mut queue = shared.queue();
        // A `finish` that gave up has already counted the batch.
        let lost = std::mem::take(&mut queue.in_flight);
        if written.is_err() {
            shared.dropped.fetch_add(lost, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/console/mod.rs"]
mod tests;
