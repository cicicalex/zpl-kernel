# A register the compiler was allowed to choose

*How one line of inline assembly, written by an AI agent and approved by four
green gates, made my kernel report a pointer as its CPU count — and why nothing
caught it for 143 days.*

---

## The bug, in one paragraph

An `x86-64` kernel needs `CPUID` to ask the processor about itself. Inline
assembly cannot bind an output to `rbx`, because the compiler reserves it, so the
usual workaround saves `rbx`, runs `cpuid`, copies `EBX` into a scratch register
and restores `rbx`. If the scratch register is requested with a constraint that
lets the register allocator choose freely, the allocator may choose `rbx` itself.
Then the copy assembles to `mov %ebx,%ebx` — a no-op — and the `pop %rbx` one
instruction later overwrites the result. The function returns whatever the caller
happened to leave in `rbx`. In a kernel, that is usually the low half of a
pointer.

Mine reported `apic_id=128 max_logical=17`. That is `0x8011…`: a kernel address,
printed as a CPU count.

---

## The code, and who wrote it

Committed on 6 May 2026, in `crates/zpl-kernel/src/smp.rs`:

```rust
unsafe {
    asm!(
        "push rbx",
        "cpuid",
        "mov {b:e}, ebx",
        "pop rbx",
        b = lateout(reg) ebx_out,
        inout("eax") eax,
        inout("ecx") ecx_out,
        lateout("edx") edx_out,
        options(preserves_flags),
    );
}
```

Above it, a comment explaining exactly why the workaround is necessary — and
getting the workaround wrong:

> LLVM reserves `rbx` for its own bookkeeping, so we cannot ask the inline-asm
> framework to bind a Rust variable to it. The workaround is to save / restore
> `rbx` ourselves around the `cpuid` instruction, and copy the result into a free
> register we *can* bind.

The reasoning is correct up to the last four words. `lateout(reg)` does not mean
"a free register"; it means "any general-purpose register you like, chosen after
the inputs are dead". `rbx` is general-purpose. `rbx` is one the allocator may
like.

The commit is `33d1633` in the working repository this kernel was written in.
That history is not what was published -- this repository starts from a single
commit -- so the hash is a label here, not something to look up. Its message, in
full:

```
feat(luna6): CPUID-based SMP topology probe — task #36 (partial)

Verified: cargo ✓ clippy ✓ test ✓ qemu ✓
Auto-loop iteration

Co-authored-by: Cursor <cursoragent@cursor.com>
```

Most of this kernel was written by AI coding agents working from a task list. The
trailer says which one wrote this, and the line above it says what it checked.
Four gates. All four green. The bug went in under them.

I am not making a point about AI agents being careless here. A human writing this
workaround makes the same mistake, and the comment shows the reasoning was
*nearly* right — which is exactly the kind of near-right that review misses. The
point is about the gates.

---

## The disassembly

```
1: 106ad7  push  %rbx          <- save
   106ad8  cpuid               <- ebx = CPUID.01H:EBX
   106ada  mov   %ebx,%ebx     <- the "copy out", a no-op
   106adc  pop   %rbx          <- result destroyed
   106ae5  mov   %ebx,%ebp     <- reads the restored, stale value
```

Five instructions. The third one is the whole bug and it is visible at a glance
— *if* you are looking at the disassembly. Nobody was.

`cpuid(0)` in the same function happened to get a different register
(`mov %ebx,%esi`), so `max_leaf` was always correct. The bug was confined to leaf
1's `EBX`: `max_logical_cpus` and `apic_id`.

---

## Why four green gates were green

This is the part worth the article.

**`cargo build`, `clippy`.** The source is valid Rust and the assembly is valid
assembly. There is nothing to warn about: asking for any register is a legal
thing to ask for. The compiler did precisely what it was told.

**Unit tests.** They ran on the host, where `cpuid` returns the host's real
values through a different code path. The bare-metal path is the one with the
wrapper in it, and a host test cannot execute it.

**The QEMU boot test.** It asserted that the marker was present and parsable. It
was: `[ZPL-SMP max_leaf=13 max_logical=17 apic_id=128 OK]`. Seventeen logical
CPUs is not an absurd number. Nothing in the gate knew what the right answer was,
so nothing could tell it apart from a wrong one.

**The determinism gate** — the one I would have bet on — compares two boots of
the same binary line by line and requires them to be identical. They were.

That last one is the lesson, and it took me a while to say it in one sentence:

> **A stale register is perfectly stable.** A pointer is the same on every boot of
> the same binary, so a check that two runs agree will agree, happily, forever.
> Determinism tests reproducibility. It does not test correctness, and I had been
> reading one as evidence of the other.

## It was not invisible. It was explained away.

Here is the part I would rather not write.

The determinism work *did* produce a signal. `max_logical` was observed as
**17 / 18 / 17 across three different binaries** in the same QEMU invocation — a
value that should be a property of the machine, changing when the code changed.
That was noticed. It was written into the test record. And this is what was
written next to it:

> the inline-asm wrapper is correct (it saves and restores `rbx`)

An anomaly was found, a plausible explanation was produced for why it was not
worth chasing, and the note was filed. The note was wrong. The value tracked the
binary because *a pointer tracks the binary*, and it was stable within one build
for the same reason.

The gates did not fail me here. I did. The signal was in the record for four
months with a sentence next to it saying not to worry.

---

## How it was actually found

Not on the hardware, and I want to be precise about that because the shape of the
story invites the wrong version.

On 26 September 2026 a separate pass went looking for something else entirely and
disassembled the shipped v0.4 ISO. It reported the `mov %ebx,%ebx`. I did not
take that on trust: I checked it in the source, then in the disassembly of the
binary I had in front of me, and only then believed it. 143 days after the commit
that introduced it.

What the hardware contributed was the reason there was a shipped artifact to
disassemble at all.

---

## What it cost

The kernel's boot is summarised by a hash over its marker lines, and the same
binary produced the same value under QEMU and on a 2009 laptop booted from USB:
`e44a58ef45488dc4`. A real result — the software path is deterministic and runs
to completion on real silicon.

The `[ZPL-SMP …]` line is inside that hashed set. Two of its three fields came
from the bug, which means they came from the *binary*, not from the hardware.
They were identical on any machine, and they contributed **nothing** to the
match.

So the claim had to shrink. It used to be the natural reading: the kernel runs on
real hardware and everything it reports agrees with the emulator. What it can
honestly be is narrower:

> The software path is deterministic and runs to completion on real hardware,
> producing byte-identical output. It is *not*: every value the kernel read from
> the hardware was confirmed. Two of them were not read from the hardware at all.

`max_leaf=13`, on the same line, was genuinely read and genuinely had to match.
One hardware value is corroborated. Two were never hardware values.

The v0.4 artefacts were not rebuilt. The ISO, the logs and the photographs stay
exactly what was tested, and the record now carries the correction next to the
result rather than in place of it.

---

## The fix, and the gate that would have caught it

The fix is to stop hand-writing the workaround:

```rust
use core::arch::x86_64::__cpuid;

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    // SAFETY: CPUID is unprivileged and has no side effects. Leaf 0 is
    // architecturally guaranteed; callers check `max_leaf` before
    // trusting any higher leaf.
    let r = unsafe { __cpuid(leaf) };
    (r.eax, r.ebx, r.ecx, r.edx)
}
```

which assembles to a wrapper that cannot pick `rbx` for the copy, because it does
not use a copy at all:

```
mov  %rbx,%r10
cpuid
xchg %rbx,%r10
```

QEMU now prints `[ZPL-SMP max_leaf=13 max_logical=0 apic_id=0 OK]`. Zero is the
correct value for a single-processor QEMU guest.

The new gate reads **the disassembly, not the source** — `scripts/cpuid-clobber-check.ps1`.
It finds every `cpuid` in the built artifact and reports two patterns in the
instructions that follow:

- **SELF-MOVE** — a `mov %R,%R` in the window. Nobody writes a register to itself
  on purpose here.
- **LOST-EBX** — a `pop %rbx` with no earlier instruction in the window copying
  `ebx` into a *different* register. The result is discarded.

On the current binary:

```
==> cpuid-clobber-check on target\x86_64-zpl-kernel\release\zpl-kernel-bin
    instructions: 23605   cpuid sites: 2
cpuid-clobber-check: PASS (2 site(s), EBX copied out before rbx is restored)
```

### The positive control is the shipped artifact, and it has to be

A gate that has never failed is a gate nobody has tested. So the repository keeps
a verbatim disassembly window from the binary as it shipped, and the gate is run
against it:

```
SUSPECT SITES:
  [SELF-MOVE] cpuid at index 13, +1 instruction(s): movl %ebx, %ebx
  [LOST-EBX]  cpuid at index 13, +1 instruction(s): movl %ebx, %ebx

[E_CPUID_010_CLOBBER] 2 finding(s) at 1 site(s).
```

Why a stored fixture rather than rebuilding the broken source? Because **the
broken source does not reliably produce the broken binary.** A rebuild on 27
September, from the same buggy code, allocated `r10d`/`r11d` and came out
*correct*.

Sit with that one. The bug was a register-allocation coin flip. Reviewing the
source would not show it. Rebuilding and checking would have told me it was fine.
The only artifact that reliably contains the bug is the one that actually shipped
with it — which is why that window is checked into the repository and will stay
there.

---

## What I take from it

Four things, and I am deliberately not making them bigger than they are.

1. **Determinism gates test reproducibility.** Stability and correctness are
   different properties and I was reading one as the other. A gate that compares
   two runs can only ever tell you the code is consistent with itself.
2. **Some bugs only exist in the artifact.** If the compiler is allowed to make a
   choice, the source does not fully determine the program, and checking the
   source cannot be enough. Disassembling what you ship is not paranoia; it is
   the only place that class of bug is visible.
3. **An unexplained value is a finding, not a footnote.** The 17/18 anomaly was
   observed, recorded and dismissed with a plausible sentence. The plausible
   sentence was the failure.
4. **Fixture your positive controls from reality.** When a bug is
   non-deterministic, a synthetic reproduction is a guess. The artifact that had
   it is the only honest test of the gate that catches it.

## What this does not show

The gate catches one shape of one bug in one instruction. It is not a general
check for inline-assembly constraint mistakes, and there is no reason to think
`CPUID` is the only place in this kernel where a hand-written constraint is
wrong. I found this one because somebody disassembled the binary for an unrelated
reason.

I also do not know how many bugs the four gates *did* catch, because a gate that
works produces no artefact. The honest summary is not "AI-written code is
dangerous" or "automated checks work". It is narrower: these checks caught a
great deal, this is the one that got through, and here is exactly why.

---

## Check it yourself

Everything above is in the repository.

| what | where |
|---|---|
| the commit that introduced it | `33d1633`, 6 May 2026 |
| the commit that fixed it | `f550cfe`, 26 September 2026 |
| the full account, with what the claim cost | `docs/V04_TEST_RECORD.md` §2.1 and §2.2 |
| the gate | `scripts/cpuid-clobber-check.ps1` |
| the positive control | `tests/fixtures/cpuid-clobber-prefix.txt` |

```bash
powershell scripts/cpuid-clobber-check.ps1           # the binary today: PASS
powershell scripts/cpuid-clobber-check.ps1 -Disassembly tests/fixtures/cpuid-clobber-prefix.txt
                                                     # the artifact that had it: exit 1
```

<REPO URL>
