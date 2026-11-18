// Copyright 2026 BoxLite Contributors
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::sync::mpsc::{Receiver, sync_channel};

const PORT: u32 = 2695;
const PREFACE: &[u8] = b"BOXLITE-MOUNT/1\n";

fn routed(capacity: usize) -> (Temp, Harness, Receiver<UnixStream>) {
    let dir = Temp::new();
    let (sender, receiver) = sync_channel(capacity);
    let device = Vsock::bind(
        &crate::VsockConfig {
            cid: 3,
            listen: vec![(PORT, dir.path("box.sock"))],
            connect: vec![],
        },
        Some(crate::VsockControl {
            port: PORT,
            guest_prefix: b'P',
            control_prefix: b'B',
            connections: sender,
        }),
    )
    .unwrap();
    let mut h = Harness::new(vec![]);
    h.device.reset().unwrap();
    h.device = device;
    h.activate();
    (dir, h, receiver)
}

fn connect(dir: &Temp) -> UnixStream {
    let socket = UnixStream::connect(dir.path("box.sock")).unwrap();
    bounded(&socket);
    socket
}

fn assert_closed(socket: &mut UnixStream) {
    let mut byte = [0];
    match socket.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        result => panic!("expected closed connection: {result:?}"),
    }
}

#[test]
fn one_bound_listener_routes_guest_and_split_control_without_consuming_prefaces() {
    let (dir, mut h, receiver) = routed(128);
    let mut control = connect(&dir);
    control.write_all(&PREFACE[..1]).unwrap();
    h.tick();
    let mut received = receiver.try_recv().expect("first byte dispatches control");
    received.set_nonblocking(false).unwrap();
    bounded(&received);
    control.write_all(&PREFACE[1..]).unwrap();
    let mut preface = vec![0; PREFACE.len()];
    received.read_exact(&mut preface).unwrap();
    assert_eq!(preface, PREFACE);
    assert!(h.packets.is_empty(), "control never requests a guest flow");

    let mut guest_stream = connect(&dir);
    let guest_preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    guest_stream.write_all(guest_preface).unwrap();
    let request = h.next(REQUEST).header;
    assert_eq!(request.dst_port, PORT);
    h.send(Packet {
        header: guest(RESPONSE, request.dst_port, request.src_port),
        payload: vec![],
    });
    assert_eq!(h.next(RW).payload, guest_preface);
    assert!(receiver.try_recv().is_err());
}

#[test]
fn idle_unknown_and_eof_connections_do_not_block_control_or_guest() {
    let (dir, mut h, receiver) = routed(128);
    let mut idle = connect(&dir);
    let mut unknown = connect(&dir);
    unknown.write_all(b"?").unwrap();
    let eof = connect(&dir);
    drop(eof);
    h.tick();
    assert_closed(&mut unknown);
    assert!(h.packets.is_empty());
    let mut control = connect(&dir);
    control.write_all(b"B").unwrap();
    h.tick();
    assert!(receiver.try_recv().is_ok());
    idle.write_all(b"P").unwrap();
    assert_eq!(h.next(REQUEST).header.dst_port, PORT);
}

#[test]
fn reset_and_drop_close_pending_sockets_and_release_registrations() {
    let (dir, mut h, _receiver) = routed(128);
    let mut idle = connect(&dir);
    h.tick();
    assert_eq!(h.registrar.len(), 3, "listener, reschedule, pending stream");
    h.device.reset().unwrap();
    assert_eq!(h.registrar.len(), 0);
    assert_closed(&mut idle);
    h.activate();
    let mut idle = connect(&dir);
    h.tick();
    let registrar = h.registrar.clone();
    drop(h);
    assert_eq!(registrar.len(), 0);
    assert_closed(&mut idle);
    assert!(!dir.path("box.sock").exists());
}

#[test]
fn full_and_disconnected_control_channels_close_only_the_rejected_connection() {
    let (dir, mut h, receiver) = routed(1);
    let mut first = connect(&dir);
    first.write_all(b"B").unwrap();
    h.tick();
    let mut rejected = connect(&dir);
    rejected.write_all(b"B").unwrap();
    h.tick();
    assert_closed(&mut rejected);
    let accepted = receiver.try_recv().unwrap();
    drop(accepted);
    drop(receiver);
    let mut rejected = connect(&dir);
    rejected.write_all(b"B").unwrap();
    h.tick();
    assert_closed(&mut rejected);
    let mut guest_stream = connect(&dir);
    guest_stream.write_all(b"P").unwrap();
    assert_eq!(h.next(REQUEST).header.dst_port, PORT);
}

#[test]
fn pending_sockets_share_the_guest_connection_limit() {
    let (dir, mut h, receiver) = routed(128);
    let mut clients = Vec::new();
    for _ in 0..backend::MAX_CONNECTIONS {
        clients.push(connect(&dir));
        h.tick();
    }
    assert_eq!(h.registrar.len(), backend::MAX_CONNECTIONS + 2);
    let mut queued = connect(&dir);
    queued.write_all(b"B").unwrap();
    h.tick();
    assert!(
        receiver.try_recv().is_err(),
        "pending sockets reserve the cap"
    );
    clients[0].write_all(b"B").unwrap();
    h.tick();
    drop(receiver.try_recv().unwrap());
    h.tick();
    assert!(receiver.try_recv().is_ok(), "routing releases capacity");
}

#[test]
fn control_routing_leaves_other_listen_ports_raw() {
    let dir = Temp::new();
    let (sender, receiver) = sync_channel(128);
    let device = Vsock::bind(
        &crate::VsockConfig {
            cid: 3,
            listen: vec![
                (PORT, dir.path("box.sock")),
                (PORT + 1, dir.path("raw.sock")),
            ],
            connect: vec![],
        },
        Some(crate::VsockControl {
            port: PORT,
            guest_prefix: b'P',
            control_prefix: b'B',
            connections: sender,
        }),
    )
    .unwrap();
    let mut h = Harness::new(vec![]);
    h.device.reset().unwrap();
    h.device = device;
    h.activate();
    let _idle_raw = UnixStream::connect(dir.path("raw.sock")).unwrap();
    assert_eq!(h.next(REQUEST).header.dst_port, PORT + 1);
    assert!(receiver.try_recv().is_err());
}
