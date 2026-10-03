# Architecture

How a request travels through this kernel, what decides it, and what the parts
that decide it do and do not guarantee.

Written for someone reading the source for the first time. Every claim here is
either visible in the code or stated as a limitation; where something is
weaker than its name suggests, the section says so rather than leaving it for
the reader to find out.

**The policy engine is not in this repository.** What is here is the gateway it
plugs into and a demo policy that takes its place, so the kernel runs and the
gate can be watched working. Nothing below describes the engine.

---

## 1. The shape of it

```
   ring 3     a program                    int 0x80
  ─────────────────│──────────────────────────│──────────────────────────
   ring 0          ▼                          ▼
              syscall dispatch  ──▶  the policy gateway  ──▶  ALLOW
                    │                                          DEGRADE
                    │                                          BLOCK
                    ▼
              the rest of the kernel
              paging · frames · heap · ramfs · pipes · shm · scheduler
```

Two kinds of caller reach the gate:

- **user programs**, through `zpl_compute` (syscall 1);
- **the kernel itself**, in two places — the scheduler, once per task per timer
  tick, and the shared-memory path, before a write is carried out.

Both go through the same three functions, so there is one place that decides and
one place to read to find out how.

| crate | what it is |
|---|---|
| `zpl-kernel` | the kernel: memory, rings, syscalls, filesystem, IPC, scheduler, the policy gateway and the decision log |
| `zpl-kernel-bin` | the bare-metal binary, for QEMU's `-kernel` |
| `limine-bridge` | the same kernel behind the Limine boot protocol, for an ISO |
| `zpl-asm` | a small assembler for the `.zpla` example programs |
| `zpl-fs`, `zpl-mkfs` | an on-disk filesystem format and the tool that makes an image |
| `audit-replay` | reads a decision log back and checks the chain |
| `zpl-test-runner` | boots the kernel in QEMU and asserts on the serial output |

---

## 2. A system call, end to end

```mermaid
sequenceDiagram
    autonumber
    participant P as ring-3 program
    participant I as IDT vector 0x80
    participant D as syscall::dispatch
    participant G as zpl_policy (gateway)
    participant K as the kernel

    P->>I: int 0x80  (rax=1, rdi=&ComputeInput)
    Note over I: DPL=3 trap gate, so ring 3 may raise it;<br/>the CPU switches to the ring-0 stack from the TSS
    I->>D: rax, rdi, rsi, rdx captured at entry
    D->>G: policy_compute(input)
    G-->>D: ComputeOutput { score, status }
    D->>G: evaluate(config, &output)
    G-->>D: Allow | Degrade | Block
    D->>K: carry the request out, or refuse it
    D-->>P: iretq, rax = (decision << 8) | score_pct
```

The order matters and is the whole point: **the gate answers before the kernel
acts**, never after. A refused request is not performed and then logged; it is
not performed.

### What the caller gets back

`rax` packs both halves of the answer, so a program can act on it without a
second call:

```
  bits 15..8   decision   0 = Allow, 1 = Degrade, 2 = Block
  bits  7..0   score      0..100
```

### The shapes crossing the boundary

```rust
#[repr(C)]
struct ComputeInput  { bias: f64, dimension: u32, samples: u32, seed: u64 }   // 24 bytes
#[repr(C)]
struct ComputeOutput { ain: f64, deviation: f64, p_output: f64, status: u8 }  // 32 bytes
```

These are the ABI. They are identical whether the private engine or the demo
policy is linked, which is what lets one be swapped for the other without a
single caller changing.

`docs/SYSCALL_ABI.md` has all ten syscall numbers and their arguments.

---

## 3. The demo policy

In `crates/zpl-kernel/src/zpl_policy.rs`, and small enough to read in full.

A request declares one number, its `bias`, between 0 and 1. The demo reads that
as *how far this request pushes*, and scores it `1 - bias`. Three rules:

| # | when | verdict |
|---|---|---|
| 1 | score at or above `DEMO_ALLOW_SCORE` | `Allow` |
| 2 | score below `DEMO_BLOCK_SCORE` | `Block` |
| 3 | after `DEMO_REQUESTS_PER_BOOT` requests in one boot, anything that would have been allowed is capped into the degrade band instead | `Degrade` |

A request that was already below the block line stays blocked; the quota does not
rescue it.

**These are demonstration values and not measurements.** They are deliberately
round so that nobody mistakes them for one. The demo policy exists so that a
kernel built from this source runs and the gate can be watched deciding; it is
not a model of anything.

Three unit tests assert one rule each, and they go through the pure form of the
score function, so none of them depends on how many requests another test
happened to make first.

---

## 4. Getting to ring 3

The kernel does this twice: once for its own demo programs, and once for
anything an ELF loader hands it.

```
  1. allocate two frames                    frame_alloc::alloc_frame
  2. map them user-accessible (U=1)         paging::map_4k_user_page
       code page    0x4001_0000
       stack page   0x4002_0000
  3. copy the program in
  4. push ss, rsp, rflags, cs, rip          cs and ss with RPL 3
  5. iretq
```

From there the only way back into the kernel is `int 0x80`. The IDT entry for it
is a trap gate with DPL 3, so ring 3 is allowed to raise it; the CPU takes the
ring-0 stack from `rsp0` in the TSS, which is why the TSS has to be installed
first.

### The three demo programs

The public build runs three small programs this way at the end of its boot, one
per rule:

| program | asks for | rule | verdict |
|---|---|---|---|
| `clean` | one modest request | 1 | `ALLOW` |
| `hostile` | a request far over the limit | 2 | `BLOCK` |
| `flood` | the same modest request, 400 times | 3 | `ALLOW`, then `DEGRADE` |

**They are three programs, not three processes.** There is no `fork` yet, so they
share one address space and run one after another from a small runner. The claim
is only what the ABI supports: ring 3, `int 0x80`, and a verdict per request.

After `hostile` is refused it does not then attempt the write. That is the point
of a gate — the answer is acted on, not merely recorded.

---

## 5. The decision log, and what it is not

`crates/zpl-kernel/src/audit/audit_chain.rs`. Each record carries a mix of its
predecessor's hash, so the chain can be re-walked from a fixed genesis value and
every link checked:

```
  genesis ──▶ record 0 ──▶ record 1 ──▶ record 2 ──▶ …
              prev_hash    prev_hash    prev_hash
              self_hash ───┘ self_hash ──┘
```

```rust
#[repr(C)]
struct AuditRecord {
    seq: u64, tick: u64, tid: u32, ain_pct: u8, decision: u8,
    prev_hash: u64, self_hash: u64,
}
```

`verify_chain` re-hashes from genesis and reports the index of the first record
that does not match. A boot exercises this with five records, including a
deliberate tamper that the check is required to catch:

```
[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK
```

### Four things it does not do

Stated here rather than left to be discovered, because the words *hash chain* and
*audit log* both promise more than this delivers today.

1. **Live decisions are not in it.** The five records above are the ones the
   self-check writes. The scheduler, `zpl_compute` and the shared-memory path do
   **not** append — they emit a marker on the serial port and that is all. So a
   boot that makes four hundred decisions ends with five records in the chain.
   Wiring the gate to the log is the obvious next step and it has not been taken.
2. **The hash is not cryptographic.** It is a deterministic bit mixer: fast,
   constant-time, and not collision-resistant. It detects accidental corruption.
   It does not detect a deliberate rewrite by anyone who can also recompute the
   chain — which is anyone who can write the records.
3. **Nothing is signed.** There is no key and no signature, so a record does not
   attest to who produced it.
4. **It is in RAM.** The chain does not survive a reboot. The on-disk format and
   the tool that reads it (`zpl-fs`, `audit-replay`) exist and work on files, but
   the kernel does not yet write one.

What the chain *is* good for today: proving that a sequence of records has not
been disturbed since it was written, in one run, in memory. That is a real
property and a narrow one.

---

## 6. Boot, and what you can check

```
  QEMU -kernel ──▶ multiboot ──▶ kernel_entry ──▶ self-checks ──▶ ring-3 demos ──▶ halt
       ISO      ──▶ Limine    ──┘                                (qemu_boot only)
```

Everything the kernel does is announced on COM1 as a one-line marker. The screen
gets the same lines, minus the per-phase timing block, so on the `-kernel` path:
*screen = serial minus the `[ZPL-PERF …]` lines*. `docs/MARKERS.md` lists every
marker and what it means.

Two gates read that output, and both can be run by hand:

```bash
bash scripts/ci-boot-check.sh boot.log   # boots it, checks the markers, no FAIL
bash scripts/demo-policy-test.sh boot.log  # the three rules are all visible
```

---

## 7. What is deliberately not here

- **The policy engine.** The gateway's two functions are what the kernel calls;
  the demo policy is what answers them in this build.
- **`fork`, and therefore real processes.** The process table exists; the ring-3
  context switch that would make it useful does not.
- **A network stack.** The virtio-net probe enumerates PCI and reports what it
  finds; nothing sends a packet.
- **Persistence for the decision log.** See §5.

`docs/KNOWN_LIMITATIONS.md` is the longer list.
