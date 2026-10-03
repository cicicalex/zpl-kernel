# Security Policy

## What this is

A research kernel. Nobody should be running anything they care about on
it, and it makes no security guarantees beyond the ones written down
here. The most useful thing to know before reading further is in
**Known weaknesses** below: the decision log is not tamper-evident
against anyone who can write the file, and the code says so too.

## Reporting a vulnerability

Bugs that let user space escape ring 3, corrupt kernel memory, or bypass
the policy gate are taken seriously.

Please **do not** open a public issue for those. Instead:

1. Email `contact@zeropointlogic.io` with the subject `ZPL kernel security`.
2. Include a reproduction: the commit SHA plus a minimal program or a
   QEMU serial log.
3. Allow up to 7 days for a first response and 30 days to agree a
   disclosure timeline.

## Scope

In scope:

- Ring-3 to ring-0 privilege escalation in `zpl-kernel`.
- A syscall path that mutates state without emitting the documented
  decision marker — that is, a policy gate that can be skipped.
- Memory-safety bugs in the allocator, the paging code, or the HAL.
- Any way to make the decision log disagree with what the kernel
  actually did, **other than** the known weaknesses listed below.

Out of scope:

- Host-side tooling (`zpl-asm`, `zpl-test-runner`) where kernel
  behaviour is unaffected.
- Third-party crates — please file those upstream.

## Known weaknesses

These are already known. Reporting them is welcome but will not be
treated as a new finding.

- **The decision log is not cryptographically protected.** Records are
  linked with a 64-bit non-cryptographic, unkeyed mixer. It detects
  accidental corruption. It does **not** survive deliberate tampering:
  anyone who can write the file can recompute the whole chain and it
  will verify clean. The source says this in its own comments, and it is
  a design stage rather than an oversight — but "audit chain" here means
  a linked log, not a tamper-proof one.
- **The log records outcomes, not inputs.** A record commits to the
  decision that was made, not to the request that produced it. So the
  log cannot be used to show that a particular decision followed from a
  particular input.
- **The computation core is not in this repository.** The public build
  links a small demo policy in its place, with three rules written out
  in `crates/zpl-kernel/src/zpl_policy.rs`: a request within the demo
  budget is allowed, a request over the demo limit is refused, and
  after a per-boot quota further requests are throttled. Its values are
  demonstration values. A kernel built from this source boots, runs
  programs and shows the decision boundary working — but the decisions
  are the demo policy's, not the engine's, and nothing here should be
  read as a statement about the engine.

## Disclosure history

None yet.
