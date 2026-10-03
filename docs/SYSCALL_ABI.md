# ZPL Kernel Syscall ABI v1

Status: **draft v1**, kernel v0.3 era. See `crates/zpl-kernel/src/syscall.rs`
for the dispatcher and `crates/zpl-kernel/src/interrupts.rs` for the
interrupt-gate plumbing.

## Calling convention

Syscalls are issued from ring 3 via the software interrupt:

```
int 0x80
```

The IDT vector `0x80` is configured with DPL=3 so user mode can invoke it
without a `#GP`. Convention:

| Register | Role on entry              | Role on exit          |
|----------|----------------------------|-----------------------|
| `RAX`    | syscall number (`SYS_*`)   | return value (`u64`)  |
| `RDI`    | argument 0                 | preserved             |
| `RSI`    | argument 1                 | preserved             |
| `RDX`    | argument 2                 | preserved             |
| `R10`    | argument 3                 | preserved             |
| `R8`     | argument 4                 | preserved             |
| `R9`     | argument 5                 | preserved             |
| Other    | preserved                  | preserved             |

`RFLAGS` is restored by `iretq` from the trap frame; the kernel does not
clobber user `RFLAGS`.

Pointers passed across the boundary refer to the user virtual address
space (currently the 4 KiB-paged user region under `paging::USER_REGION_BASE`).
The kernel reads/writes user memory via the same identity-mapped physical
backing (no copy_to/from_user yet — see `Out of scope` below).

## Syscall numbers

| Number | Name              | Args                            | Return                                  | Status   |
|--------|-------------------|---------------------------------|-----------------------------------------|----------|
| 0      | `zpl_log`         | `RDI=ptr, RSI=len`              | `0` on success                          | live     |
| 1      | `zpl_compute`     | `RDI=in_ptr, RSI=out_ptr`       | `0` on success                          | stub     |
| 2      | `zpl_alloc`       | `RDI=size`                      | virt addr (`0` if no mem)               | stub     |
| 3      | `zpl_yield`       | -                               | `0`                                     | stub     |
| 4      | `zpl_exit`        | `RDI=code`                      | does not return; see the note below     | live     |
| 5      | `zpl_read_audit`  | `RDI=buf, RSI=len`              | bytes copied                            | stub     |
| 6      | `zpl_set_policy`  | `RDI=policy_id`                 | `0` on accept                           | stub     |
| 7      | `zpl_open`        | `RDI=path_ptr, RSI=flags`       | fd or `~0u64` on error                  | stub     |
| 8      | `zpl_read`        | `RDI=fd, RSI=buf, RDX=len`      | bytes read                              | stub     |
| 9      | `zpl_write`       | `RDI=fd, RSI=buf, RDX=len`      | bytes written                           | stub     |

**`zpl_exit` and the demo programs.** In every build, `zpl_exit` ends the caller
and does not return. What happens next depends on who called it:

- normally it shuts the machine down through `isa-debug-exit`, which is what the
  scenario gates assert on;
- in the public `-kernel` build, while the three ring-3 demo programs are
  running, it hands control back to the kernel instead. The boot still has a halt
  loop to enter and a panel to leave on the screen, and shutting down at that
  point would mean a reader never sees either. The condition is a flag the demo
  sets and clears, so no other caller is affected.


`stub` syscalls log a `[ZPL-SYSCALL] stub num=<N>` line on COM1 and
return `0`; they exist so user libraries can be wired up in parallel with
their kernel implementation.

## Error model

For v1, syscalls return either a positive value or `~0u64`
(`0xFFFF_FFFF_FFFF_FFFF`) for unrecoverable errors. A richer `errno`-style
return space is reserved for v2 once user space libraries demand it.

## Out of scope (deferred)

- Copy-to/from-user with bounds checks (currently the kernel trusts that
  pointers point into the user-mapped region; ring-3 cannot reach kernel
  memory anyway because `PDPT[0]` is supervisor-only).
- Signal delivery on syscall return.
- Pre-emption / yield with full scheduler queue (not yet wired).
- File descriptors with multiple backing types (waiting on filesystem
  tasks in Luna 3).

## Reference user program (ring 3)

The kernel ring-3 demo at `crates/zpl-kernel/src/demo/ring3_demo.rs` constructs
a 24-byte program at user virt `0x40010000`:

```
mov eax, 0          ; SYS_LOG
mov edi, 0x40010800 ; ptr to "hi"
mov esi, 2          ; len
int 0x80
mov eax, 4          ; SYS_EXIT
mov edi, 0          ; exit code
int 0x80
```

The string `"hi"` is placed at user virt `0x40010800`. A successful run
emits on COM1:

```
[ZPL-SYSCALL] log: hi
[ZPL-SYSCALL] exit code=0
```

after which QEMU shuts down via the existing `isa-debug-exit` path.
