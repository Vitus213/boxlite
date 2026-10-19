# boxlite-vmm

BoxLite's native VMM. `Vm::new` validates the machine configuration, creates
the host VM and registers guest RAM. The crate-visible VM and vCPU `run` entry
points still sketch event dispatch, guest exits, and worker cleanup with inline
`todo!()` operations that panic if called; vCPU exits use
`boxlite_hypervisor::VcpuExit`, and `Error` wraps `boxlite_hypervisor::Error`
with its cause chain intact. The address buses and the device interrupt path
are implemented, as are the x86_64 serial console and i8042 reset; the lifecycle
does not register them yet. This crate cannot boot a kernel yet. The
[VMM design](../../docs/contributing/architecture/vmm/README.md) specifies the
lifecycle API, memory layout, buses, interrupts, and threads it will implement.

`Error`, `Result`, `bus` and `irq` are public so device slices and M2's engine
adapter can use them. The lifecycle types `Vm` and `VmExit` stay crate-visible
until the lifecycle task implements them; M2's engine adapter will then consume
that API and inspect the error's host cause.

```text
boxlite-vmm
├── boxlite-hypervisor
├── vm-memory
└── vm-superio (x86_64)
```

| Module | Planned responsibility |
| --- | --- |
| `vm` | `Vm<H>`: validate the configuration, then allocate and register RAM on host backend `H`; `run` is a placeholder |
| `config` | `VmConfig { vcpu_count, memory_mib }` and its bounds: 1..=64 vCPUs, 1..=3072 MiB |
| `error` | VMM errors that keep the hypervisor's cause chain |
| `memory` | `GuestRam`: owned guest RAM, the RAM layout, and the registration lifetime rules |
| `vcpu` | Worker threads, stop coordination, and exit handling |
| `irq` | `IrqSender`, the device interrupt path, over any backend `Vm`; HVF and KVM provide the controller, WHP (M10) only local APICs |
| `bus` | `Bus` (MMIO) and, on x86_64, `IoBus` (ports): window registration and device I/O dispatch |
| `devices` | x86_64 legacy devices: the 8250 serial console and the i8042 reset |

Backend implementation and guest boot follow in M1. Virtio devices, the BoxLite
engine adapter, engine selection, and `native` feature wiring follow in M2.
Neither new crate depends on `boxlite-shared`, and both are unpublished while
their interfaces are being established.

## Buses and interrupts

Devices implement `BusDevice` and see offsets from their window's base.
`insert` rejects a zero-sized window, a window that runs past the end of its
address space (`InvalidWindow`), and any overlap (`Overlap`, or `IoOverlap` on
the port bus); adjacent windows are allowed. An access must land wholly inside
one window: a split or unmapped access returns `Unmapped`/`IoUnmapped`, and a
poisoned device lock returns `DevicePoisoned`. Whether the vCPU loop treats
those as fatal or ignores them, as Firecracker and crosvm do for stray port
I/O, is decided with the loop. A vCPU thread holds a device's lock for one
register access.

`IrqSender` wraps any `InterruptTarget`; every `boxlite_hypervisor::Vm`
implements it, so the lifecycle hands its backend VM to devices directly.
`set_level` holds a level-triggered line; `trigger_edge` is a set followed by a
clear and reports a failed clear rather than hiding a stuck line.

## x86_64 legacy devices

M1 reuses `vm-superio` 0.8.2 for the protocols and keeps only BoxLite's side
in `devices`: bus offsets, interrupt delivery and reset reporting.

| Device | Ports | BoxLite adapter |
| --- | --- | --- |
| 8250 serial (16550A) | `0x3F8`–`0x3FF`, GSI 4 | output to a `Write` sink; `vm-superio` signals each new interrupt once, so `IrqTrigger` sends an edge; `enqueue_input` feeds the receive FIFO |
| i8042, reset only | one window at `0x60`: data at offset 0, command at offset 4 | `0xFE` on the command port sets the reset flag through `ResetTrigger`; reads are `0` |

No guest keyboard is configured in M1, so the i8042 emulates no keyboard
protocol. The reset is a flag, not a stop: the vCPU loop checks it right after
dispatching each port write, before re-entering the guest, because Linux
otherwise falls back to port `0xCF9` and a triple fault. A port write has no
error path back to the guest, so a failing console sink or interrupt injection
drops that byte or edge.

## Guest RAM

`memory::ram_ranges` places one region: at guest address 0 on x86_64, below
the 32-bit MMIO hole at `0xC000_0000`, and at `0x8000_0000` on arm64. The
3072 MiB bound keeps every layout inside that region; the region above 4 GiB
and the arm64 address-space limit are later changes.

`GuestRam` allocates the layout as zero-filled anonymous memory through
vm-memory and registers it with the host VM. Three rules keep the backend's
`map_memory` contract:

- Host-side users clone the shared `Arc<GuestMemoryMmap>` and access guest
  memory through vm-memory's volatile API, never through Rust references.
- `map` registers regions lowest address first and removes the registered
  prefix when one fails; `unmap` removes them highest first and stops at the
  first failure, so the count of registered regions stays exact and a later
  `unmap` retries.
- The backing is released only after every registration is gone. Dropping a
  `GuestRam` that may still be registered leaks the backing instead.

`Vm::with_backend` runs validate, allocate, create the host VM, then map, so
every early return drops the host VM before the RAM; `Vm`'s `Drop` unmaps as a
backstop. Unit tests drive these paths with a recording mock backend. The
`memory` and `vm` tests and that backend live in `tests/` and are included as
unit-test modules, which keeps them outside the counted sources and the
coverage denominator; `config` and `error` keep inline test modules.

## Build

`make vmm:boot` builds the pinned x86_64 ELF kernel (`vmlinux`) inside a pinned
BoxLite box; `make vmm:boot:host` uses Linux host tools instead. See
[boot](boot/README.md) for prerequisites, outputs and limits.

From the repository root:

```sh
make vmm
make clippy:vmm
make test:unit:vmm
make fmt:check:rust
```

On Linux x86_64 with read/write access to `/dev/kvm`,
`make test:integration:vmm:kvm` also runs this crate's hardware test: a VM
with the largest layout and a flat 32-bit guest that reads the last RAM byte
and reports it on port `0x3f8`. The 3 GiB allocation is `MAP_NORESERVE`, so
only the touched pages are committed; a host with `vm.overcommit_memory=2`
refuses it. CI runs the same test through `make coverage:vmm:kvm`.

With the corresponding Rust targets installed, the crate can also be compiled
for each host with a backend module without booting a VM:

```sh
CARGO_BUILD_TARGET=aarch64-apple-darwin make vmm
CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu make vmm
CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu make vmm
CARGO_BUILD_TARGET=x86_64-pc-windows-msvc make vmm
```

The existing Clippy CI matrix compiles workspace crates on the first three
hosts. No CI job compiles the Windows target, whose `whp` module is reserved
for M10.
