# Known limitations

What this kernel does not do, written down so nobody has to find out by trying.
It is a research kernel; the list is long on purpose.

## The policy engine is not here

The gateway the kernel calls is here; what answers it in this build is a demo
policy with three rules and deliberately round numbers. It runs, and it can be
watched deciding, and it is not a model of anything. See `ARCHITECTURE.md` §3.

## The decision log is weaker than its name

Four separate limits, all in `ARCHITECTURE.md` §5 at more length:

1. **Live decisions are not recorded in it.** The chain holds the five records
   its self-check writes. The scheduler, `zpl_compute` and the shared-memory path
   emit a marker on the serial port and nothing else. A boot that makes four
   hundred decisions ends with five records.
2. **The hash is not cryptographic.** It detects accidental corruption. It does
   not survive anyone who can write the records, because they can recompute the
   chain too.
3. **Nothing is signed.** No key, no signature, so a record does not attest to
   who produced it.
4. **It is in RAM.** It does not survive a reboot. The on-disk format and the
   replay tool exist and work on files; the kernel does not write one yet.

## No processes, in the sense the word usually means

There is a process table, PIDs and exit codes. There is no `fork`, because the
ring-3 context switch it needs is not written. So the three demo programs share
one address space and run one after another. Anything that says "processes"
about this kernel is saying more than the code does.

## One network card, one protocol

There is an e1000 driver. It maps the card, reads its MAC, sets up a transmit
and a receive ring, sends an ARP request and reads the reply back off the wire,
and can be told to do it from an interrupt instead of a poll. That is the whole
of it.

The heading used to end "and one boot path". It does not any more -- the driver
works on both -- and the heading is corrected rather than left to contradict the
paragraphs under it, which already said so.

What is not there: any other card, any IP stack, and any use of the received
frame beyond recognising an ARP reply. Adding a stack would mean a new
dependency, and the public build has none.

The driver works on both boot paths now. It took two separate bugs to get there,
and both were found by putting markers down the call path until the last one that
printed and the first one that did not were adjacent.

The first was the same mistake the ELF loader had: physical addresses written
through as if they were virtual, in the descriptor ring, the packet buffer and the
descriptor writes.

The second was subtler and is worth stating, because the wrong version of it is a
natural thing to write. The receive loop waits by halting the CPU, which is polite
to the emulator and costs nothing -- as long as something will wake it. The guard
asked whether interrupts were enabled. They were. It did not ask whether any
interrupt would actually arrive, and on the Limine path none does: the loop halted
on its first turn, never reached the timeout check at the top, and the boot stopped
there. "The timer was configured" and "the timer has fired" are different claims,
and only the second one is safe to halt on.

**What it costs on that path:** with nothing to wake it, the wait spins instead of
halting -- about 800,000 turns of the loop for a one-second wait, against about a
hundred where the timer runs. A core busy for a second, rather than a machine
stopped for good.

## The boot fingerprint depends on what was typed, and on one thing about line breaks

The panel's summary shows a fingerprint -- an FNV-1a hash of the marker lines this boot
has produced -- and claims it can be recomputed from the serial log with a published
filter. For a while that claim was false, by two whole lines: two modules wrote their
markers straight to the serial port, past the function the watcher observes. Measured on
one boot: the log gave `e5ebc5b32c902a94`, the screen said `6b3d394289927b14`, and dropping
exactly those two lines reproduced the screen value to the bit. Both now go through one
path, and on the same boot both values are `5eb0968d32ce03df`.

Two things about it are still worth knowing.

**It is a fingerprint of the boot, not of the build.** Decisions caused by typed commands
now count, which is what makes it match the log -- and means two boots of the same image
differ if different commands were typed. The determinism test compares automated boots
that type nothing, so it is unaffected; a person comparing two hands-on boots should not
expect them to agree.

**The equality assumes each marker starts its own line in the log.** The watcher sees
markers; the published filter keeps lines beginning with `[ZPL-`. These are the same set
only while nothing else shares the line. The prompt can do that -- a marker emitted right
after `zpl> ` with no newline between them lands on a line the filter drops but the watcher
counted. It did not happen in the boot measured above (zero such lines), and nothing
prevents it.

## A self-check used to write past the end of the kernel stack

Fixed, and recorded here because of how it was found rather than what it was.

The hardware-abstraction self-check built its two mock devices as locals. One of
them holds eight frames of 1500 bytes, so the pair came to roughly fifteen
kilobytes on a kernel stack of eight. It had been that way for a long time and did
no visible harm: what the overrun landed on did not matter.

It started mattering when a one-byte static was removed from an unrelated module.
The layout shifted, the overrun moved onto the table that records boot timings, and
twelve of its sixteen entries came back as zero while two held the bytes
`ZPL-FRAME-RX`. It was found by decoding one of the nonsense numbers in a
`[ZPL-PERF]` line back into ASCII.

Two things worth keeping from it:

- **The stack guard did not catch it.** A guard page catches a stack that grows
  into it one frame at a time. It does not catch a single frame set-up that
  reserves more than the whole stack and starts writing beyond the guard.
- **A latent memory bug is invisible until the furniture moves.** Nothing about
  the change that exposed this one was wrong; it just relaid the statics.

The mocks are static now, which is correct for a check that runs once on the boot
CPU, and takes them off the stack entirely.

## The command line is small

`zpl-sh` reads a line from the keyboard or from COM1 and answers it. Thirteen
commands. Most report the kernel's own state; `run clean` and `run hostile` put
one request through the policy gate; and `ls`, `load <file>` and `run <file>`
work on files in the in-RAM filesystem, which holds the two demo programs.

`run <file>` parses the ELF, maps it, enters ring 3, and comes back with the
program's exit code. That last part is not a context switch: it is a one-shot
jump back across the ring boundary, one program at a time, with no saved register
file and no scheduler. A program that never exits never gives the prompt back.

All thirteen work on both boot paths now -- the heading above used to end "and one
of its commands works on one boot path", which stopped being true and is corrected
rather than left standing. They did not always: on the ISO path `load` used
to stop the machine dead. The ELF loader wrote to a freshly allocated frame as if
a physical address were a usable virtual one -- true where low memory is
identity-mapped, false where the kernel runs in the higher half and reaches
physical memory through a window. One statement, found by putting markers down
the call path until the last one that printed and the first one that did not were
adjacent.

What the command line still is not: no history, no completion, no pipes, no
scripting, no job control, and no way to get a file into the filesystem other
than the two the kernel puts there at boot.

## One CPU

The SMP module reads the CPU topology through `CPUID` and reports it. It does not
start a second core. `-smp 4` boots and uses one of them.

## Not a security boundary

Ring 0 and ring 3 are separated, and the page tables mark user pages as such. But
the syscall layer trusts the pointers user space hands it — there is no
`copy_from_user`, and `docs/SYSCALL_ABI.md` says so for each call that takes one.
A hostile program in ring 3 can make the kernel read from an address of its
choosing. This is a demonstration of the architecture, not a hardened one.

## Real hardware: two machines, and neither fully works with this release

- **Acer Aspire 5738 (2009), legacy BIOS, from a USB stick.** An earlier build
  booted here and read a USB keyboard through the BIOS's USB Legacy Support
  emulation of a PS/2 keyboard. This release boots and draws, but **does not read
  the keyboard**. The laptop's built-in keyboard is faulty, so every test used a
  USB keyboard. The cause is not known yet.
- **CSL Narrow Box mini PC, UEFI, from a USB stick.** The kernel **does not
  start**: the machine falls back to its installed system. Whether the firmware
  skips the stick or the kernel resets early is not known yet.
- **QEMU** boots the same image over BIOS and over UEFI (edk2), from a CD image and
  from a raw disk image the way a stick is written. So the UEFI path works in a
  virtual machine; what fails is specific to real hardware.

A machine with a real PS/2 port is untested. `docs/V04_TEST_RECORD.md` states at
the same length what the earlier laptop test does and does not show.

## No performance claim

There are no benchmarks in this repository comparing this kernel to anything. In
particular nothing here establishes that deciding per request costs less, or
more, than not deciding, or that the scheduler's choices are better than a
simpler rule on the same inputs. `docs/V04_TEST_RECORD.md` §4 says the comparison
was not performed.

## Boot paths

Two, and they differ:

- `-kernel` with multiboot, which is the one the automated gates use;
- an ISO through Limine, which is the one that reaches real hardware.

They now draw the same policy panel, on different consoles -- the text buffer on
the first, a graphical framebuffer on the second. They do not run the same code
everywhere else: the network driver is started on the first only, and the
command line is compiled into the second only.

A change to one is not automatically a change to the other, and the gates
exercise the first far more than the second.
