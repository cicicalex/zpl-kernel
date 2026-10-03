# zero-point-kernel

<!-- CI BADGE: uncomment the line below once the repository exists, and replace
     OWNER/REPO with its real path. Left commented on purpose: a badge pointing
     at a repository that does not exist renders as a broken image, which is a
     worse first impression than no badge at all.

[![CI](https://github.com/OWNER/REPO/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/REPO/actions/workflows/ci.yml)
-->

A small x86-64 research kernel written in Rust, `no_std`. It boots under QEMU and
from a USB stick on real hardware, brings up its own paging, memory allocators,
ring 0/3 separation, system calls, an ELF loader, an in-RAM filesystem, pipes,
shared memory, a command line and a network driver -- and it runs every
state-changing request past a policy gate **before** carrying it out, rather than
auditing it afterwards.

![One boot in QEMU: the self-checks pass, three ring-3 programs each ask the
policy gate for something and are allowed, refused and throttled in turn, and the
panel at the bottom counts the answers](docs/media/boot-demo.gif)

**It has a command line.** `help` lists what it answers:

![The kernel's prompt listing its thirteen commands, with the policy panel
underneath](docs/media/shell-help.png)

**And the gate answers from it.** A balanced request is throttled because this
boot has already spent its quota; a skewed one is refused outright; and the
decision chain still verifies afterwards:

![Two requests through the gate from the prompt -- one throttled, one refused --
and the audit chain reporting five records and an intact chain](docs/media/shell-gate.png)

## Running it

```bash
cargo +nightly -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec build -p zpl-kernel-bin --release --no-default-features --features public,shell --target crates/zpl-kernel/x86_64-zpl-kernel.json
```

```bash
qemu-system-x86_64 -kernel target/x86_64-zpl-kernel/release/zpl-kernel-bin -serial stdio -display none -m 256 -no-reboot
```

That gives you the prompt on your terminal. `scripts/build-iso.ps1` builds a
bootable image instead, for a USB stick. Longer instructions, and what each
command does, are further down.

---

It is a research kernel. It is not a general-purpose operating system, it is not
a Linux competitor, and nothing important should run on it.

**The decision engine is not in this repository; a demo policy is.**

The engine that the project uses to score a request is developed separately and
is not published. Everything else is here: the kernel around it, the gate it
plugs into, and a demo policy with three readable rules that occupies the same
place behind the same ABI. That is what the `public` build links, what the
screenshots show, and what the image on a USB stick runs. A build configured for
the engine will not compile from this tree alone, which is the only practical
difference you will meet.

This is said once, here, rather than file by file.

## What the demo policy does

Three rules, written out in `crates/zpl-kernel/src/zpl_policy.rs`:

1. a request within the demo budget is allowed;
2. a request over the demo limit is refused;
3. after a per-boot quota, further requests are throttled.

The numbers are demonstration values. A single boot shows all three happening,
twice over: once from inside the kernel, when the shared-memory self-check writes
with a modest request and then a hostile one, and once from user space, where
three small ring-3 programs ask for one thing each. A panel at the bottom of the
screen counts the answers and shows the rule behind each.

## What is in here

| crate | what it is |
|---|---|
| `zpl-kernel` | the kernel: paging, frame and heap allocators, a TSS and ring-3 setup, an in-RAM filesystem, pipes, shared memory, a scheduler, the policy gateway and the decision log |
| `zpl-kernel-bin` | the bare-metal binary, for QEMU's `-kernel` |
| `limine-bridge` | the same kernel behind the Limine boot protocol, for booting from an ISO on real hardware |
| `zpl-asm` | a small assembler for the `.zpla` example programs |
| `zpl-fs`, `zpl-mkfs` | an on-disk filesystem format and the tool that makes an image |
| `audit-replay` | reads a decision log back and checks the chain |
| `zpl-test-runner` | boots the kernel in QEMU and asserts on the serial output |

Documentation worth starting with: **`docs/ARCHITECTURE.md`**, which follows a
request from ring 3 through the gate and back and says what each part does and
does not guarantee; then `docs/SYSCALL_ABI.md` for the system-call numbers,
`docs/MARKERS.md` for what every line on the serial port means, and
`docs/KNOWN_LIMITATIONS.md` for what does not work.

## Building

Needs a nightly Rust toolchain with `rust-src`, because the kernel builds the
core library for its own target.

```bash
rustup toolchain install nightly --component rust-src clippy
rustup default nightly

cargo check --workspace
cargo test --workspace --exclude zpl-kernel-bin --exclude limine-bridge

cargo -Zbuild-std=core,compiler_builtins,alloc \
      -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
      build -p zpl-kernel-bin \
      --target crates/zpl-kernel/x86_64-zpl-kernel.json --release
```

The binary lands at `target/x86_64-zpl-kernel/release/zpl-kernel-bin`.

## Booting it in QEMU

```bash
qemu-system-x86_64 \
  -kernel target/x86_64-zpl-kernel/release/zpl-kernel-bin \
  -serial file:boot.log -no-reboot -m 256
```

That is the whole demonstration: one command, no disk image, no configuration.
It takes about five seconds and then sits in its halt loop; stop it when you have
seen enough. Everything on the screen also goes to `boot.log`, apart from the
per-phase timing lines, which are serial only.

## What you see when you boot it

**The screen is in two parts.** The top rows are the boot log, scrolling. The
bottom seven are a panel that does not scroll: it is the policy gate, and it
stays where it is so you can read it while the log goes past. Its last row is
the kernel's own summary -- version, how many self-checks passed, how many
decisions have been taken, and a fingerprint of the boot.

### 1. It comes up

![The first screen: kernel_entry, the build identity, the frame allocator, and
the policy panel already drawn with every counter at zero](docs/media/boot-selfchecks.png)

The panel is there from the first frame, with all three counters at zero and all
three verdicts marked `not seen yet in this boot`. Nothing has asked the gate for
anything yet.

### 2. The self-checks run, and the gate answers its first two requests

![The self-checks passing, and the two shared-memory decisions: the clean write
allowed in green, the hostile one refused in red, with the panel showing the
refusal and the rule behind it](docs/media/decisions-allow-block.png)

Paging, the stack guard, the in-RAM filesystem, processes, pipes, shared memory,
the matrix operations and the scheduler each check themselves and say so. Two of
those go through the gate: the shared-memory self-check writes once with a modest
request and once with a hostile one. The first is allowed, the second refused,
and the panel now shows the refusal with the rule that produced it.

### 3. Three programs run in ring 3, one per rule

![Three ring-3 programs announcing what they are about to ask for, each followed
by the gate's answer: allowed, then refused](docs/media/ring3-programs.png)

Each program says what it is about to ask for, then asks, through a real
`int 0x80`:

| program | what it asks for | rule | answer |
|---|---|---|---|
| `clean` | one modest request | 1, within the budget | `ALLOW` |
| `hostile` | a request far over the limit | 2, over the limit | `BLOCK` |
| `flood` | the same modest request, 400 times | 3, past the quota | `ALLOW`, then `DEGRADE` |

The third one is the interesting one. The per-boot quota has not been reached by
the time it starts, so it has to reach it itself — which is what "asking too
often" means. Watch the lines turn from green to yellow part-way through.

They are three programs, not three processes: there is no `fork` yet, so they
share one address space and run one after another. What the demonstration claims
is only what the ABI supports — ring 3, `int 0x80`, and an answer per request.

### 4. The panel, at the end

![The panel after the demo: 168 allowed, 267 throttled, 2 refused, and the last
decision of each kind with the rule that produced it](docs/media/gate-panel.png)

Left to right: how many requests the gate allowed, throttled and refused, then
how many it has seen against the per-boot quota. Below that, the most recent
decision **of each kind** — not the last three in time, which would all be the
scheduler allowing the same task again — with where the request came from, what
it scored, and the rule that decided it.

Everything above is one boot of the command above. `docs/media/boot.log` is that
boot's serial output, so the screen and the log can be checked against each
other line by line.

To check it in one command instead of by eye:

```bash
bash scripts/demo-policy-test.sh boot.log
```

It fails if nothing was allowed or nothing was refused, and — once a boot has
made more requests than the quota — if nothing was throttled. On a log with no
decisions in it at all it fails rather than reporting success over nothing.

## The command line

The ISO build brings up a prompt after the self-checks:

```
zpl-sh. type help for the list of commands.
zpl> version
ZPL kernel v0.5.0
policy: public (demo policy, no engine)
zpl> mem
heap  1496 of 262144 bytes
frames 3 of 262144 used
zpl> run hostile
[ZPL-SH run=hostile ain=13 action=BLOCK] strongly skewed input: write refused
zpl> audit
records 5
chain intact
```

It reads the PS/2 keyboard and COM1, so it works with a screen and a keyboard or
with a serial cable and nothing else. The commands:

| | |
|---|---|
| `help` | the list |
| `version` | the kernel version, and which policy is linked in |
| `pci` | buses walked and devices found |
| `mem` | heap and physical frames in use |
| `ps` | the process table |
| `audit` | how long the chain is and whether it still verifies |
| `run clean` / `run hostile` | one request through the gate, and what it decided |
| `net` | the network card, if there is one |
| `uptime` | ticks since the halt loop started |

![The kernel answering pci, mem, ps, net and uptime in turn -- its own state, one
line per answer](docs/media/shell-state.png)
| `ls` | the files in the in-RAM filesystem |
| `load <file>` | parse a program and map it, without running it |
| `run <file>` | load a program and run it in ring 3 |

A program run from the prompt goes into ring 3 and comes back with its exit code:

```
zpl> ls
  /tmp/foo            11 bytes
  /hello.elf          151 bytes
  /attack.elf         173 bytes
zpl> run /attack.elf
/attack.elf: loaded, 1 segment at 0x40010000
[ZPL-SCHED tid=user ain=6 action=BLOCK]
[ZPL-SYSCALL] exit code=0
/attack.elf: exited with 0
```

That is the whole demonstration in four lines: a program is loaded from a
filesystem, runs in ring 3, asks the kernel for something, and is refused --
with the refusal on the same screen as the request.

Coming back is not a context switch. It is a one-shot jump back across the ring
boundary: one program at a time, no saved register file, no scheduler. A program
that never exits never gives the prompt back.

This works on both boot paths, which it did not at first: on the ISO the loader
stopped the machine, because it wrote to a newly allocated frame as if a physical
address were a usable virtual one. That is true on the path where low memory is
identity-mapped and false on the path where the kernel runs in the higher half.

There is no history, no completion and no scripting: it is a line, a word, and an
answer. What it does have is a line editor that cannot overrun its buffer, a
keyboard decoder that drops what it cannot turn into a character rather than
guessing, and answers formatted by pure functions -- which is why they are
unit-tested on a host rather than only looked at on a screen.

**`run` is the one command that changes something.** It puts a request through
the same gate the boot sequence uses, with the same two inputs, so the prompt and
the self-check cannot disagree about what "clean" and "hostile" mean.

**What you type is not in the boot fingerprint; what it causes is.** The echo of
a typed line goes to the serial port on a path the fingerprint's watcher does not
read, so the characters themselves never reach it. But a command that puts a
request through the gate produces a decision line, and decision lines are counted
like any other marker. So two boots of the same image agree only if the same
commands were typed. That is the honest behaviour -- the fingerprint is of the
boot, not of the build -- and it is what makes the number on screen equal the one
you get by hashing the serial log with the published filter. It was not always:
two modules used to write their markers straight to the port, past the watcher,
and the screen was quietly showing the log's hash minus their lines.

## Networking

The kernel talks to the network card QEMU's default machine already contains —
an Intel e1000 at `00:03.0` — and gets an answer back. What it does, end to end:

1. enumerates the PCI bus and finds the card;
2. maps the card's register window and reads the MAC address out of it;
3. turns on bus mastering, builds transmit and receive rings out of physical
   frames, and lets the card raise its interrupt;
4. sends one ARP request asking who has `10.0.2.2`;
5. receives the reply, checks it is an answer to that question, and prints it.

A boot with a card present prints this:

```
[ZPL-PCI] 00:03.0 vid=8086 did=100e class=02:00 ethernet controller bar0=0xfebc0000 irq=11
[ZPL-E1000] 00:03.0 mac=52:54:00:12:34:56 valid link=up qemu-prefix OK
[ZPL-E1000] tx arp who-has 10.0.2.2 len=60 polls=1 confirmed OK
[ZPL-E1000] rx len=64 polls=1 irqs=0 icr=0x00000000 cycles=895402 ... arp reply: 10.0.2.2 is at that address OK
```

![The end of a boot: the ARP request going out, the link and PHY reported, the
reply coming back on the first look, and the policy panel underneath counting
the boot's decisions](docs/media/network-arp.png)

`polls=1` means the reply was in the ring on the first look. The card is set up
near the top of the boot and asked its question at the very end, and the reason
is measured rather than stylistic — see below.

The card can also raise IRQ 11 rather than only being asked, and does. A build
whose boot ends with interrupts enabled shows it:

```
[ZPL-E1000] rx len=64 polls=102 irqs=1 icr=0x00000083 ... arp reply: ... OK
```

Bit 7 of that word is "a frame arrived". The handler does as little as a handler
should — it asks the card whether the interrupt was its own, records it, and
acknowledges both interrupt controllers. Reading the ring and deciding what the
frame means happens in ordinary code afterwards.

In the demo build above the answer is already in the ring before the interrupt
can be taken: that last step runs inside a system-call handler, where interrupts
are masked, so the card's interrupt stays pending and is never serviced. Hence
`irqs=0` there. Nothing is lost — the frame is read either way — but the line is
reporting what happened, not what would look better.

### The log is not the evidence

The kernel saying it sent a frame is the kernel's opinion. QEMU can write what
actually left the card to a capture file, and `tools/net/check_arp_pcap.py`
reads that file and checks it byte by byte — the addresses, the opcode, and that
all eighteen padding bytes are zero, because a frame padded with whatever was in
the buffer would put kernel memory on the wire.

```
qemu-system-x86_64 -kernel <kernel> -display none -no-reboot -m 256   -netdev user,id=n0 -device e1000,netdev=n0   -object filter-dump,id=f0,netdev=n0,file=arp.pcap
python tools/net/check_arp_pcap.py arp.pcap --expect-reply
```

The checker is itself checked: corrupting one byte of a good capture — the
opcode, a MAC address, one padding byte — makes it refuse, and those cases are
run rather than assumed.

This works under both of QEMU's accelerators on Windows, `tcg` and `whpx`.

### One measured oddity, and what it is

On the wire the reply is there 0.06–0.1 ms after the request. The kernel can
take about a second to see it, and that second is a timer in the emulated card,
started by the write that enables the receiver; until it expires the device
model accepts nothing. It is anchored to that write and not to power-on:
delaying everything from before it by a known interval leaves the wait
unchanged, and delaying only the question shortens it by exactly that interval.

Which is why the kernel arms the card early and asks late. The measurement, same
binary, question moved from just after the setup to the end of the boot:

| when the question is asked | wait | boot |
|---|---|---|
| right after arming the card | 3,189,493,531 cycles | 3.0 s |
| at the end of the boot | 895,402 cycles | 2.1 s |

About 1.06 s down to about 0.3 ms. The gain is exactly the work the kernel does
in between, so it is not free everywhere: a build with nothing between the two
points still waits the full second, and says so in the same line.

![The same boot with the console feature on: the exchange the boot made, then
the prompt, then a second request sent by pressing a key and answered on the
first look at the ring](docs/media/network-console.png)

The optional `--features net_console` build puts the same thing under a key: a
request is sent when you press one, rather than only at boot. It is not a shell
— one key, one packet, no parser — and it is off by default, because every
automated run here feeds the serial port from a file that has no input side.

On this machine the guest's cycle counter runs at about 3 GHz; the cycle counts
above are what the kernel prints and the seconds are that division.
### What this is not

There is no IP, no TCP and no network stack. ARP is the whole of it: one
question and one answer, at the level of Ethernet frames the kernel builds and
parses itself. `smoltcp` would be the obvious next step and is not a dependency
here.

## Booting on real hardware

```powershell
.\scripts\build-iso.ps1
```

`limine-bridge` plus that script produce a bootable ISO of about 18 MB. It
downloads a pinned Limine release, checks its SHA-256, and refuses to go on if
the hash does not match.

The same image runs under QEMU from the ISO, over the BIOS path a USB stick
uses:

```bash
qemu-system-x86_64 -cdrom artifacts/iso/zpl-kernel.iso -boot d \
  -serial file:iso-boot.log -no-reboot -m 256
```

Booted this way the kernel comes up on a graphical framebuffer rather than the
text buffer, with the command line and the policy panel on the same screen.

![The ISO booted over BIOS: the same self-checks on a graphical console, the
`zpl-sh` prompt with four commands typed at it, and the policy panel along the
bottom](docs/media/iso-boot.png)

### Where it has been tested

| Machine | Boot mode | Result |
|---|---|---|
| QEMU, default machine | BIOS, and UEFI with edk2 | boots and runs; the command line answers under BIOS |
| Acer Aspire 5738 laptop (2009), USB stick | legacy BIOS | boots and draws; **the keyboard is not read in this release** — an earlier build did read it on the same laptop |
| CSL Narrow Box mini PC, USB stick | UEFI | **does not start**; the machine falls back to its installed system, and the cause is not known yet |

Real-hardware support is early. Every change is tested in QEMU; real machines
are tested by hand, and the two above are all there has been so far. The laptop's
built-in keyboard is faulty, so it was tested with a USB keyboard.

`docs/V04_TEST_RECORD.md` has what the earlier laptop test checked, what passed,
and — at the same length — what that test does **not** show.

## How it was built

Most of this code was written by AI coding agents working from a task list, and
every change had to pass the automated gates under `scripts/` before it was kept.
Those gates catch a great deal; they did not catch everything, and
`docs/KNOWN_LIMITATIONS.md` lists what is known to have got through.

## Licence

GNU General Public License v3.0 only — the full text is in `LICENSE`.

In short: you may use, study, change and share this, and anything you distribute
that is built from it has to come with its source under the same licence. If that
does not suit what you want to do with it, ask.
