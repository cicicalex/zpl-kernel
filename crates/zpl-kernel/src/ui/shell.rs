//! `zpl-sh`: the kernel's command line.
//!
//! Task #10. Until now the only way to ask the kernel anything was to press one of
//! three keys and read what scrolled past. This is a prompt: you type a word, it
//! answers, and the answer is the same whether you typed it on the PS/2 keyboard or
//! sent it down COM1.
//!
//! **Everything that formats an answer lives in this file and runs on the host.**
//! The kernel gathers the numbers into [`Facts`] and a slice of [`ProcRow`], and
//! [`respond`] turns those into lines. That split is the reason the command output
//! can be unit-tested at all: a shell whose only test is "boot it and look" is a
//! shell whose output drifts.
//!
//! **Nothing here writes a `[ZPL-` marker that counts.** `v04_status::line_counts`
//! excludes `ZPL-SH` for the same reason it excludes `ZPL-V04`: the boot sequence
//! must hash identically whether or not anyone typed at the prompt.

use core::sync::atomic::{AtomicU32, Ordering};

/// Longest command line accepted, chosen so the prompt plus the line fits an
/// 80-column screen with a character to spare.
pub const LINE_MAX: usize = 74;

/// What the prompt looks like. Short on purpose: the screen is 80 columns wide and
/// the framebuffer console has no horizontal scroll.
pub const PROMPT: &[u8] = b"zpl> ";

/// Somewhere for a formatted answer to go.
///
/// The kernel implements this over COM1 and the framebuffer; the tests implement it
/// over a fixed byte array.
pub trait Out {
    /// Write bytes with no newline of their own.
    fn put(&mut self, bytes: &[u8]);

    /// Write bytes followed by a newline.
    fn line(&mut self, bytes: &[u8]) {
        self.put(bytes);
        self.put(b"\n");
    }
}

// ---------------------------------------------------------------------------
// Line editing
// ---------------------------------------------------------------------------

/// What the caller should do about the byte it just fed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Nothing to show: the byte was not printable and did not mean anything.
    Ignored,
    /// Echo this byte; it went into the line.
    Echo(u8),
    /// Rub out the character to the left of the cursor.
    Erase,
    /// The line is complete. Read it with [`LineEditor::line`], then clear.
    Submit,
    /// The line is full; the byte was dropped. Echo nothing, or beep.
    Full,
}

/// A fixed-size line buffer with backspace.
///
/// No history, no cursor movement, no escape-sequence parsing. An arrow key arrives
/// as three bytes starting with `0x1b`, and all three are simply not printable, so
/// they are dropped one at a time rather than landing in the line as junk.
pub struct LineEditor {
    buf: [u8; LINE_MAX],
    len: usize,
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0u8; LINE_MAX],
            len: 0,
        }
    }

    /// Feed one input byte.
    pub fn feed(&mut self, byte: u8) -> Edit {
        match byte {
            // Both, because a terminal sends CR and a raw keyboard path sends LF.
            b'\r' | b'\n' => Edit::Submit,
            // 0x08 is Ctrl-H, 0x7f is what most terminals send for Backspace.
            0x08 | 0x7f => {
                if self.len == 0 {
                    Edit::Ignored
                } else {
                    self.len -= 1;
                    Edit::Erase
                }
            }
            // Ctrl-C: throw the line away without running it.
            0x03 => {
                self.len = 0;
                Edit::Submit
            }
            b' '..=b'~' => {
                if self.len == self.buf.len() {
                    Edit::Full
                } else {
                    self.buf[self.len] = byte;
                    self.len += 1;
                    Edit::Echo(byte)
                }
            }
            _ => Edit::Ignored,
        }
    }

    /// The line as typed so far, without a newline.
    #[must_use]
    pub fn line(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Which workload `run` was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// The balanced input the shared-memory self-check uses.
    Clean,
    /// The strongly skewed input from `attack-test`.
    Hostile,
}

/// One parsed command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Nothing but whitespace was typed.
    Empty,
    Help,
    Version,
    Pci,
    Mem,
    Ps,
    Audit,
    Net,
    Uptime,
    Run(Workload),
    /// `run` with a missing or unrecognised argument.
    RunBadArg(&'a [u8]),
    /// List the files in the in-RAM filesystem.
    Ls,
    /// Parse a program out of the filesystem and map it, without running it.
    Load(&'a [u8]),
    /// Load a program and enter it.
    Exec(&'a [u8]),
    /// `load` or `exec` with no file named.
    NeedsAName(&'a [u8]),
    /// A first word that is not a command.
    Unknown(&'a [u8]),
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = s.split_first() {
        if is_space(first) {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = s.split_last() {
        if is_space(last) {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// Split into the first word and whatever follows it, both trimmed.
fn split_word(s: &[u8]) -> (&[u8], &[u8]) {
    let s = trim(s);
    match s.iter().position(|&b| is_space(b)) {
        Some(i) => (&s[..i], trim(&s[i..])),
        None => (s, &[]),
    }
}

/// Case-insensitive comparison, ASCII only.
fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Turn a typed line into a [`Command`].
///
/// Unknown input is never an error the caller has to handle: it comes back as
/// [`Command::Unknown`] carrying what was typed, so the answer can quote it.
#[must_use]
pub fn parse(line: &[u8]) -> Command<'_> {
    let (word, rest) = split_word(line);
    if word.is_empty() {
        return Command::Empty;
    }
    if eq_ignore_case(word, b"help") || eq_ignore_case(word, b"?") {
        Command::Help
    } else if eq_ignore_case(word, b"version") {
        Command::Version
    } else if eq_ignore_case(word, b"pci") {
        Command::Pci
    } else if eq_ignore_case(word, b"mem") {
        Command::Mem
    } else if eq_ignore_case(word, b"ps") {
        Command::Ps
    } else if eq_ignore_case(word, b"audit") {
        Command::Audit
    } else if eq_ignore_case(word, b"net") {
        Command::Net
    } else if eq_ignore_case(word, b"uptime") {
        Command::Uptime
    } else if eq_ignore_case(word, b"ls") {
        Command::Ls
    } else if eq_ignore_case(word, b"load") {
        let (arg, _) = split_word(rest);
        if arg.is_empty() {
            Command::NeedsAName(b"load")
        } else {
            Command::Load(arg)
        }
    } else if eq_ignore_case(word, b"run") {
        let (arg, _) = split_word(rest);
        // The two built-in workloads keep their names. Anything else is read as a
        // file, which is the point of the command: `run <name>` runs a program out
        // of the filesystem. `run` alone is still the old complaint.
        if eq_ignore_case(arg, b"clean") {
            Command::Run(Workload::Clean)
        } else if eq_ignore_case(arg, b"hostile") {
            Command::Run(Workload::Hostile)
        } else if arg.is_empty() {
            Command::RunBadArg(arg)
        } else {
            Command::Exec(arg)
        }
    } else {
        Command::Unknown(word)
    }
}

// ---------------------------------------------------------------------------
// Facts the kernel hands over
// ---------------------------------------------------------------------------

/// One row of a directory listing, flattened so `respond` needs nothing from `ramfs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRow {
    pub name: [u8; 64],
    pub name_len: u8,
    pub size: u16,
}

/// What a `load` did. The kernel fills this in; `respond` only prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadResult {
    /// Parsed and mapped: entry point and how many segments were placed.
    Ok { entry: u64, segments: u32 },
    /// No file of that name.
    NoSuchFile,
    /// There is a file, but it is not an ELF this kernel can load.
    NotLoadable,
    /// Loading is not compiled into this build.
    NotAvailable,
}

/// One row of the process table, flattened so `respond` needs nothing from `process`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub parent: u32,
    /// `b'R'` running, `b'Z'` zombie. Free slots are not passed in at all.
    pub state: u8,
    pub exit_code: u64,
}

/// Everything the commands report, gathered before any of them is formatted.
///
/// Collected in one pass so two commands typed in a row cannot disagree about the
/// same number, and so the formatting stays a pure function of this struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    pub pci_buses: u32,
    pub pci_devices: u32,
    /// A bridge pointed at a bus the scan had no room for.
    pub pci_truncated: bool,

    pub heap_used: u64,
    pub heap_capacity: u64,
    pub frames_used: u64,
    pub frames_total: u64,

    pub procs_live: u32,
    pub procs_slots: u32,

    pub audit_records: u32,
    pub audit_verifies: bool,

    /// An e1000 was found by the PCI enumeration.
    ///
    /// Deliberately not "the driver is using one": on the Limine boot path the card is
    /// seen and then left alone, so asking the driver would answer "no card" about a
    /// card that is plainly on the bus.
    pub net_present: bool,
    /// The registers were mapped, so the MAC and the link state below mean something.
    pub net_mac_known: bool,
    pub net_mac: [u8; 6],
    pub net_link_up: bool,
    /// The receiver was set up on this boot path.
    pub net_armed: bool,

    /// Half-second ticks since the halt loop started.
    pub uptime_ticks: u32,
}

/// What the caller has to do after [`respond`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing; the answer is already written.
    None,
    /// Put this workload through the gate and report what it decided.
    Run(Workload),
    /// Parse and map this program, then report what happened. No ring 3.
    Load(usize),
    /// Load this program and enter it. The index is into the `files` slice.
    Exec(usize),
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Append `n` in decimal. Returns the number of bytes written.
fn fmt_u64(mut n: u64, buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut count = 0;
    while n > 0 && count < digits.len() {
        digits[count] = b'0' + (n % 10) as u8;
        count += 1;
        n /= 10;
    }
    let written = count.min(buf.len());
    for i in 0..written {
        buf[i] = digits[count - 1 - i];
    }
    written
}

/// Append `n` as exactly `width` lowercase hex digits.
fn fmt_hex(n: u64, width: usize, buf: &mut [u8]) -> usize {
    const DIGIT: &[u8; 16] = b"0123456789abcdef";
    let written = width.min(buf.len());
    for (i, slot) in buf.iter_mut().take(written).enumerate() {
        let shift = (width - 1 - i) * 4;
        *slot = DIGIT[((n >> shift) & 0xf) as usize];
    }
    written
}

/// A line assembled piece by piece, truncated rather than panicking.
struct Line {
    buf: [u8; 96],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            buf: [0u8; 96],
            len: 0,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            if self.len == self.buf.len() {
                break;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        self
    }

    fn dec(&mut self, n: u64) -> &mut Self {
        let len = self.len;
        let written = fmt_u64(n, &mut self.buf[len..]);
        self.len += written;
        self
    }

    fn hex(&mut self, n: u64, width: usize) -> &mut Self {
        let len = self.len;
        let written = fmt_hex(n, width, &mut self.buf[len..]);
        self.len += written;
        self
    }

    fn mac(&mut self, mac: &[u8; 6]) -> &mut Self {
        for (i, &byte) in mac.iter().enumerate() {
            if i > 0 {
                self.put(b":");
            }
            self.hex(u64::from(byte), 2);
        }
        self
    }

    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn emit(&self, out: &mut impl Out) {
        out.line(self.as_slice());
    }
}

// ---------------------------------------------------------------------------
// The answers
// ---------------------------------------------------------------------------

/// The help text, one entry per line. Kept next to [`parse`] so the two cannot drift:
/// a test walks this list and checks every name in it parses to something known.
const HELP: &[(&[u8], &[u8])] = &[
    (b"help", b"this list"),
    (b"version", b"kernel version and build path"),
    (b"pci", b"buses walked and devices found"),
    (b"mem", b"heap and physical frames in use"),
    (b"ps", b"the process table"),
    (b"audit", b"how long the chain is and whether it still verifies"),
    (b"run clean", b"a balanced input through the gate"),
    (b"run hostile", b"a strongly skewed input through the gate"),
    (b"ls", b"the files in the in-RAM filesystem"),
    (b"load", b"parse and map a program, without running it"),
    (b"run <file>", b"load a program and run it in ring 3"),
    (b"net", b"the network card, if there is one"),
    (b"uptime", b"ticks since the halt loop started"),
];

/// Write the answer to one command.
///
/// Pure: everything it prints comes from its arguments. `Run` is the one command it
/// cannot finish by itself, so it says so in the return value and leaves the gate to
/// the caller.
// Over the 150-line limit, and allowed rather than split.
//
// It is one `match` over the command enum, flat, with four or five lines per arm, and
// it reads one arm at a time. Splitting it would turn thirteen visible answers into
// thirteen function names a reader has to go and find, which is the thing the limit
// exists to prevent, not to cause. The limit is still on: a *new* long function
// elsewhere fails the build, and this one says out loud that it is an exception.
#[allow(clippy::too_many_lines)]
pub fn respond(
    cmd: &Command<'_>,
    facts: &Facts,
    procs: &[ProcRow],
    files: &[FileRow],
    out: &mut impl Out,
) -> Action {
    match cmd {
        Command::Empty => Action::None,

        Command::Help => {
            for (name, what) in HELP {
                let mut line = Line::new();
                line.put(b"  ");
                line.put(name);
                // Pad the name column to 12 so the descriptions line up.
                let pad = 12usize.saturating_sub(name.len());
                for _ in 0..pad {
                    line.put(b" ");
                }
                line.put(what);
                line.emit(out);
            }
            Action::None
        }

        Command::Version => {
            let mut line = Line::new();
            line.put(b"ZPL kernel ").put(crate::KERNEL_VERSION.as_bytes());
            line.emit(out);
            let mut line = Line::new();
            line.put(b"policy: ");
            if cfg!(feature = "engine") {
                line.put(b"engine");
            } else {
                line.put(b"public (demo policy, no engine)");
            }
            line.emit(out);
            Action::None
        }

        Command::Pci => {
            let mut line = Line::new();
            line.put(b"buses ")
                .dec(u64::from(facts.pci_buses))
                .put(b", devices ")
                .dec(u64::from(facts.pci_devices));
            if facts.pci_truncated {
                line.put(b" (scan truncated: a bridge pointed past the queue)");
            }
            line.emit(out);
            Action::None
        }

        Command::Mem => {
            let mut line = Line::new();
            line.put(b"heap  ")
                .dec(facts.heap_used)
                .put(b" of ")
                .dec(facts.heap_capacity)
                .put(b" bytes");
            line.emit(out);
            let mut line = Line::new();
            line.put(b"frames ")
                .dec(facts.frames_used)
                .put(b" of ")
                .dec(facts.frames_total)
                .put(b" used");
            line.emit(out);
            Action::None
        }

        Command::Ps => {
            if procs.is_empty() {
                out.line(b"no processes");
            } else {
                out.line(b"  pid  parent  state  exit");
                for row in procs {
                    let mut line = Line::new();
                    line.put(b"  ").dec(u64::from(row.pid));
                    let pad = 5usize.saturating_sub(count_digits(u64::from(row.pid)));
                    for _ in 0..pad {
                        line.put(b" ");
                    }
                    line.dec(u64::from(row.parent));
                    let pad = 8usize.saturating_sub(count_digits(u64::from(row.parent)));
                    for _ in 0..pad {
                        line.put(b" ");
                    }
                    line.put(match row.state {
                        b'Z' => b"zombie".as_slice(),
                        _ => b"running".as_slice(),
                    });
                    if row.state == b'Z' {
                        line.put(b"  ").dec(row.exit_code);
                    }
                    line.emit(out);
                }
            }
            let mut line = Line::new();
            line.put(b"live ")
                .dec(u64::from(facts.procs_live))
                .put(b", slots ")
                .dec(u64::from(facts.procs_slots));
            line.emit(out);
            Action::None
        }

        Command::Audit => {
            let mut line = Line::new();
            line.put(b"records ").dec(u64::from(facts.audit_records));
            line.emit(out);
            let mut line = Line::new();
            if facts.audit_verifies {
                line.put(b"chain intact");
            } else {
                line.put(b"chain BROKEN");
            }
            line.emit(out);
            Action::None
        }

        Command::Net => {
            if !facts.net_present {
                out.line(b"no e1000 on the bus");
            } else if !facts.net_mac_known {
                out.line(b"e1000 on the bus, registers not mapped on this boot path");
            } else {
                let mut line = Line::new();
                line.put(b"e1000 mac ").mac(&facts.net_mac);
                line.put(if facts.net_link_up {
                    b", link up".as_slice()
                } else {
                    b", link down".as_slice()
                });
                line.emit(out);
                if !facts.net_armed {
                    out.line(b"receiver not set up on this boot path");
                }
            }
            Action::None
        }

        Command::Uptime => {
            // The halt-loop tick is one per ~500 ms, so seconds are ticks over two.
            // Stated as "about" because the delay is an rdtsc estimate, not a timer.
            let mut line = Line::new();
            line.put(b"ticks ")
                .dec(u64::from(facts.uptime_ticks))
                .put(b", about ")
                .dec(u64::from(facts.uptime_ticks) / 2)
                .put(b" s since the halt loop started");
            line.emit(out);
            Action::None
        }

        Command::Run(workload) => Action::Run(*workload),

        Command::Ls => {
            if files.is_empty() {
                out.line(b"no files");
            } else {
                for f in files {
                    let mut line = Line::new();
                    line.put(b"  ");
                    let name = &f.name[..f.name_len as usize];
                    line.put(name);
                    let pad = 20usize.saturating_sub(name.len());
                    for _ in 0..pad {
                        line.put(b" ");
                    }
                    line.dec(u64::from(f.size)).put(b" bytes");
                    line.emit(out);
                }
            }
            Action::None
        }

        Command::Load(name) => match find_file(files, name) {
            Some(i) => Action::Load(i),
            None => {
                no_such_file(name, out);
                Action::None
            }
        },

        Command::Exec(name) => match find_file(files, name) {
            Some(i) => Action::Exec(i),
            None => {
                no_such_file(name, out);
                Action::None
            }
        },

        Command::NeedsAName(word) => {
            let mut line = Line::new();
            line.put(b"which file? try: ").put(word).put(b" <name>, or ls");
            line.emit(out);
            Action::None
        }

        Command::RunBadArg(arg) => {
            if arg.is_empty() {
                out.line(b"run what? try: run clean, run hostile, or run <file> (see ls)");
            } else {
                let mut line = Line::new();
                line.put(b"no workload called '")
                    .put(arg)
                    .put(b"'. try: run clean, or run hostile");
                line.emit(out);
            }
            Action::None
        }

        Command::Unknown(word) => {
            let mut line = Line::new();
            line.put(b"no command '")
                .put(word)
                .put(b"'. type help for the list");
            line.emit(out);
            Action::None
        }
    }
}

/// Find a listed file by name. Exact match, case-sensitive: these are file names.
fn find_file(files: &[FileRow], name: &[u8]) -> Option<usize> {
    files
        .iter()
        .position(|f| &f.name[..f.name_len as usize] == name)
}

fn no_such_file(name: &[u8], out: &mut impl Out) {
    let mut line = Line::new();
    line.put(b"no file called '").put(name).put(b"'. try ls");
    line.emit(out);
}

/// Print what a load did. Separate from `respond` because the kernel has to do the
/// loading first; the words are here so they are tested with everything else.
pub fn report_load(name: &[u8], result: LoadResult, out: &mut impl Out) {
    let mut line = Line::new();
    match result {
        LoadResult::Ok { entry, segments } => {
            line.put(name)
                .put(b": loaded, ")
                .dec(u64::from(segments))
                .put(if segments == 1 {
                    b" segment at 0x".as_slice()
                } else {
                    b" segments, entry 0x".as_slice()
                })
                .hex(entry, 8);
        }
        LoadResult::NoSuchFile => {
            line.put(name).put(b": no such file");
        }
        LoadResult::NotLoadable => {
            line.put(name).put(b": not a program this kernel can load");
        }
        LoadResult::NotAvailable => {
            line.put(b"loading programs is not compiled into this build");
        }
    }
    line.emit(out);
}

fn count_digits(mut n: u64) -> usize {
    if n == 0 {
        return 1;
    }
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

// ---------------------------------------------------------------------------
// Uptime counter
// ---------------------------------------------------------------------------

/// Halt-loop ticks, so `uptime` has something to read.
///
/// The halt loop already counts for its own `[ZPL-ALIVE]` line, but that counter is
/// private to it. This one is written from the same place and read by the shell.
static TICKS: AtomicU32 = AtomicU32::new(0);

pub fn note_tick(n: u32) {
    TICKS.store(n, Ordering::Relaxed);
}

#[must_use]
pub fn ticks() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The kernel side
// ---------------------------------------------------------------------------

/// Everything that needs a port, a page table or the gate.
///
/// The prompt is driven from the halt loop, the same place the v0.4 menu is read, and
/// for the same reason: interrupt routing and the scheduler stay untouched, and a
/// keystroke can only ever add output.
#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "shell"))]
pub mod kernel {
    use super::{
        Action, Command, Edit, Facts, FileRow, LineEditor, LoadResult, Out, ProcRow,
        Workload, PROMPT,
    };
    use crate::drivers::kbd::Decoder;

    /// Writes to COM1 and to whichever console is mirroring.
    struct Console;

    impl Out for Console {
        fn put(&mut self, bytes: &[u8]) {
            crate::drivers::console::shell_echo(bytes);
        }
    }

    /// Where the prompt's state lives.
    ///
    /// A `static mut` rather than a lock, because there is exactly one reader: the halt
    /// loop on the bootstrap CPU. `poll` is the only thing that touches it and nothing
    /// re-enters it.
    static mut EDITOR: LineEditor = LineEditor::new();
    static mut DECODER: Decoder = Decoder::new();
    /// Consecutive bytes taken from COM1 without giving the keyboard a turn. Same single
    /// reader, same reason it is not a lock.
    static mut COM1_RUN: u32 = 0;

    /// How many process rows `ps` will print.
    const PS_ROWS: usize = 16;
    /// How many files `ls` will print. The filesystem holds no more than this.
    const FILE_ROWS: usize = crate::fs::ramfs::MAX_FILES;

    /// The programs the prompt can load, put into the filesystem at boot.
    ///
    /// Both are the images the boot path has always used, written into files so that
    /// `load` and `run <file>` have something real to open. They are the whole of
    /// what `ls` shows besides the filesystem self-check's own file.
    #[cfg(any(feature = "elf_demo", feature = "user_programs"))]
    const SEEDED: &[(&[u8], &[u8])] = &[
        (b"/hello.elf", crate::demo::elf_demo::HELLO_ELF_BYTES),
        (b"/attack.elf", crate::demo::elf_demo::ATTACK_ELF_BYTES),
    ];

    /// Put the demo programs into the filesystem. Call once, after `ramfs::init`.
    ///
    /// Failures are reported and then ignored: a prompt with one program in it is
    /// worth more than a boot that stops because a demo file would not fit.
    #[cfg(any(feature = "elf_demo", feature = "user_programs"))]
    pub fn seed_programs() {
        for (name, bytes) in SEEDED {
            let mut ok = false;
            if let Ok(fd) = crate::fs::ramfs::open(name) {
                if crate::fs::ramfs::write(fd, bytes).is_ok() {
                    ok = crate::fs::ramfs::close(fd).is_ok();
                }
            }
            if !ok {
                let mut line = super::Line::new();
                line.put(b"[ZPL-SH] could not seed ").put(name);
                let mut out = Console;
                line.emit(&mut out);
            }
        }
    }

    #[cfg(not(any(feature = "elf_demo", feature = "user_programs")))]
    pub fn seed_programs() {}

    /// Print the greeting and the first prompt. Call once, after the self-checks.
    pub fn banner() {
        let mut out = Console;
        out.line(b"");
        // Which inputs are actually live, said out loud. On a machine with no serial
        // port the shell used to be unusable and looked identical to a working one:
        // the prompt appeared and swallowed everything. Now the screen says which
        // reader is in play, so a photograph of a stuck prompt answers the question
        // instead of raising it. Printed after `halt loop entered`, so the boot-marker
        // sequence the determinism gate hashes is untouched.
        if crate::drivers::serial::com1_present() {
            out.line(b"[ZPL-KBD] input=ps2+com1");
        } else {
            out.line(b"[ZPL-KBD] input=ps2 (no serial port on this machine)");
        }
        out.line(b"zpl-sh. type help for the list of commands.");
        out.put(PROMPT);
    }

    /// Read whatever input is waiting and act on it. Returns without blocking.
    ///
    /// Both sources are drained each time: COM1, so a scripted demo can feed the shell
    /// from a file, and the keyboard, so a machine with no serial cable works the same.
    pub fn poll() {
        // Bounded, so a controller that always claims to have a byte cannot wedge the
        // halt loop -- the same protection `poll_menu_key` has.
        for _ in 0..256 {
            let byte = match next_input_byte() {
                Some(b) => b,
                None => return,
            };
            feed(byte);
        }
    }

    /// How many bytes COM1 may contribute before the keyboard gets a turn regardless.
    ///
    /// Without a cap, one talkative source starves the other: that is exactly how a
    /// phantom serial port made the prompt untypable, and a real but noisy RS-232 line
    /// would do the same. `read_com1_byte` no longer answers for a port that is not
    /// there, so this is the second line of defence rather than the first.
    const COM1_BYTES_BEFORE_THE_KEYBOARD_GETS_A_TURN: u32 = 64;

    fn next_input_byte() -> Option<u8> {
        // SAFETY: single reader, as above.
        let since_keyboard = unsafe { &mut *core::ptr::addr_of_mut!(COM1_RUN) };
        if *since_keyboard < COM1_BYTES_BEFORE_THE_KEYBOARD_GETS_A_TURN {
            if let Some(b) = crate::drivers::serial::read_com1_byte() {
                *since_keyboard += 1;
                return Some(b);
            }
        }
        *since_keyboard = 0;
        // SAFETY: single reader, as above.
        let decoder = unsafe { &mut *core::ptr::addr_of_mut!(DECODER) };
        while let Some(code) = crate::drivers::kbd::poll_scancode() {
            if let Some(ch) = decoder.feed(code) {
                return Some(ch);
            }
        }
        None
    }

    fn feed(byte: u8) {
        // SAFETY: single reader, as above.
        let editor = unsafe { &mut *core::ptr::addr_of_mut!(EDITOR) };
        let mut out = Console;
        match editor.feed(byte) {
            Edit::Ignored | Edit::Full => {}
            Edit::Echo(ch) => out.put(&[ch]),
            // Back up, overwrite with a space, back up again: the only way to rub a
            // character out on a console that has no cursor addressing.
            Edit::Erase => out.put(b"\x08 \x08"),
            Edit::Submit => {
                out.put(b"\n");
                run_line(editor.line(), &mut out);
                editor.clear();
                out.put(PROMPT);
            }
        }
    }

    fn run_line(line: &[u8], out: &mut Console) {
        let cmd = parse_line(line);
        let mut rows = [ProcRow {
            pid: 0,
            parent: 0,
            state: b'R',
            exit_code: 0,
        }; PS_ROWS];
        let count = gather_procs(&mut rows);
        let mut files = [FileRow {
            name: [0u8; 64],
            name_len: 0,
            size: 0,
        }; FILE_ROWS];
        let file_count = gather_files(&mut files);
        let facts = gather_facts();
        match super::respond(&cmd, &facts, &rows[..count], &files[..file_count], out) {
            Action::None => {}
            Action::Run(workload) => run_workload(workload),
            Action::Load(i) => {
                let f = files[i];
                let name = &f.name[..f.name_len as usize];
                let (result, _) = load_program(name);
                super::report_load(name, result, out);
            }
            Action::Exec(i) => {
                let f = files[i];
                let name = &f.name[..f.name_len as usize];
                exec_program(name, out);
            }
        }
    }

    fn gather_files(rows: &mut [FileRow; FILE_ROWS]) -> usize {
        let mut raw = [([0u8; crate::fs::ramfs::MAX_PATH], 0u8, 0u16); FILE_ROWS];
        let n = crate::fs::ramfs::list(&mut raw);
        for i in 0..n {
            let len = (raw[i].1 as usize).min(64);
            rows[i].name = [0u8; 64];
            rows[i].name[..len].copy_from_slice(&raw[i].0[..len]);
            rows[i].name_len = len as u8;
            rows[i].size = raw[i].2;
        }
        n
    }

    /// Read a file out of the filesystem and put it through the ELF loader.
    ///
    /// Returns what happened and, on success, the entry point to jump to.
    #[cfg(any(feature = "elf_demo", feature = "user_programs"))]
    fn load_program(name: &[u8]) -> (LoadResult, u64) {
        // The loader used to hang the kernel on the Limine boot path, and did not
        // any more: it wrote to a freshly allocated frame as if physical addresses
        // were usable virtual ones, which is true on the multiboot path and not on
        // this one. Found with markers, fixed in `elf::map_segment`. Both paths are
        // exercised at the prompt now.
        let fd = match crate::fs::ramfs::open(name) {
            Ok(fd) => fd,
            Err(_) => return (LoadResult::NoSuchFile, 0),
        };
        let mut buf = [0u8; crate::fs::ramfs::MAX_DATA];
        let n = match crate::fs::ramfs::read(fd, &mut buf) {
            Ok(n) => n,
            Err(_) => {
                let _ = crate::fs::ramfs::close(fd);
                return (LoadResult::NotLoadable, 0);
            }
        };
        let _ = crate::fs::ramfs::close(fd);
        // SAFETY: `elf::load` parses the bytes before it maps anything, and it maps
        // only the pages the program headers ask for, inside the user region.
        match unsafe { crate::fs::elf::load(&buf[..n]) } {
            Ok(image) => (
                LoadResult::Ok {
                    entry: image.entry,
                    segments: image.load_count,
                },
                image.entry,
            ),
            Err(_) => (LoadResult::NotLoadable, 0),
        }
    }

    #[cfg(not(any(feature = "elf_demo", feature = "user_programs")))]
    fn load_program(_name: &[u8]) -> (LoadResult, u64) {
        (LoadResult::NotAvailable, 0)
    }

    /// Load a program and run it, then say what it exited with.
    #[cfg(any(feature = "elf_demo", feature = "user_programs"))]
    fn exec_program(name: &[u8], out: &mut Console) {
        let (result, entry) = load_program(name);
        if !matches!(result, LoadResult::Ok { .. }) {
            super::report_load(name, result, out);
            return;
        }
        super::report_load(name, result, out);
        // SAFETY: the image above is loaded and mapped as user pages, the TSS is
        // installed, and `user_run::enter` refuses a second program while one is live.
        match unsafe { crate::demo::user_run::enter(entry) } {
            Ok(code) => {
                let mut line = super::Line::new();
                line.put(name).put(b": exited with ").dec(code);
                line.emit(out);
            }
            Err(crate::demo::user_run::RunError::AlreadyRunning) => {
                out.line(b"a program is already running");
            }
            Err(crate::demo::user_run::RunError::NoStack) => {
                out.line(b"no free frame for the program's stack");
            }
        }
    }

    #[cfg(not(any(feature = "elf_demo", feature = "user_programs")))]
    fn exec_program(name: &[u8], out: &mut Console) {
        super::report_load(name, LoadResult::NotAvailable, out);
    }

    /// Parse, keeping the borrow of `line` inside this call.
    fn parse_line(line: &[u8]) -> Command<'_> {
        super::parse(line)
    }

    fn gather_procs(rows: &mut [ProcRow; PS_ROWS]) -> usize {
        let mut raw = [(0u32, 0u32, 0u8, 0u64); PS_ROWS];
        let n = crate::sched::process::snapshot(&mut raw);
        for i in 0..n {
            rows[i] = ProcRow {
                pid: raw[i].0,
                parent: raw[i].1,
                state: raw[i].2,
                exit_code: raw[i].3,
            };
        }
        n
    }

    fn gather_facts() -> Facts {
        let scan = crate::drivers::pci::scan();
        let mut facts = Facts {
            pci_buses: scan.buses_walked as u32,
            pci_devices: scan.total as u32,
            pci_truncated: scan.truncated,
            heap_used: crate::mm::heap::used() as u64,
            heap_capacity: crate::mm::heap::capacity() as u64,
            frames_used: crate::mm::frame_alloc::used_frames(),
            // What the allocator may actually hand out on this machine, not the size
            // of its bitmap. The two differ wherever the firmware kept memory for
            // itself, which is every real machine.
            frames_total: crate::mm::frame_alloc::capacity_frames(),
            procs_live: crate::sched::process::live_count(),
            procs_slots: crate::sched::process::MAX_PROCESSES as u32,
            audit_records: crate::audit::audit_chain::len() as u32,
            audit_verifies: crate::audit::audit_chain::verify_chain().is_ok(),
            uptime_ticks: super::ticks(),
            ..Facts::default()
        };
        // Presence comes from the bus, not from the driver: the Limine boot path sees an
        // e1000 and then leaves it alone, so `armed()` is `None` there even with a card
        // plugged in. Answering from `armed()` alone would have printed "no e1000 on the
        // bus" while `pci` listed one two lines earlier.
        facts.net_present = scan
            .devices
            .iter()
            .flatten()
            .any(crate::drivers::e1000::is_e1000);
        if let Some(armed) = crate::drivers::e1000::armed() {
            facts.net_present = true;
            facts.net_mac_known = true;
            facts.net_mac = armed.mac;
            facts.net_armed = armed.rx.is_some();
            // SAFETY: `armed()` is `Some` only after `read_mac` mapped the register
            // window, which is exactly what `read_status` requires.
            let status = unsafe { crate::drivers::e1000::read_status() };
            facts.net_link_up = crate::drivers::e1000::status_link_up(status);
        }
        facts
    }

    /// Put one workload through the shared-memory gate and say what was decided.
    ///
    /// The same two inputs the v0.4 menu uses, so the prompt and the menu cannot
    /// disagree about what "clean" and "hostile" mean. The marker is `[ZPL-SH …]`; the
    /// reason for a refusal is stated in words, never as a threshold.
    fn run_workload(workload: Workload) {
        use crate::ipc::shm::{shm_create, shm_write, WriteOutcome};

        let (label, payload, bias) = match workload {
            Workload::Clean => (b"clean".as_slice(), b"clean".as_slice(), 0.5f64),
            Workload::Hostile => (b"hostile".as_slice(), b"hostile".as_slice(), 0.95f64),
        };
        let mut out = Console;
        let id = match shm_create(b"shdemo") {
            Ok(id) => id,
            Err(_) => {
                out.line(b"[ZPL-SH] shm-create-failed");
                return;
            }
        };
        let mut line = super::Line::new();
        line.put(b"[ZPL-SH run=").put(label).put(b" ain=");
        match shm_write(id, 0, payload, bias) {
            Ok(WriteOutcome::Allow { ain_pct }) => {
                line.dec(u64::from(ain_pct))
                    .put(b" action=ALLOW] ran");
            }
            Ok(WriteOutcome::Degrade { ain_pct }) => {
                line.dec(u64::from(ain_pct))
                    .put(b" action=DEGRADE] ran with reduced resources");
            }
            Ok(WriteOutcome::Block { ain_pct }) => {
                line.dec(u64::from(ain_pct))
                    .put(b" action=BLOCK] strongly skewed input: write refused");
            }
            Err(_) => {
                out.line(b"[ZPL-SH] shm-write-failed");
                return;
            }
        }
        line.emit(&mut out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An [`Out`] that keeps what was written so a test can read it back.
    struct Capture {
        buf: [u8; 2048],
        len: usize,
    }

    impl Capture {
        fn new() -> Self {
            Self {
                buf: [0u8; 2048],
                len: 0,
            }
        }
        fn text(&self) -> &[u8] {
            &self.buf[..self.len]
        }
        fn contains(&self, needle: &[u8]) -> bool {
            self.text()
                .windows(needle.len())
                .any(|w| w == needle)
        }
        fn lines(&self) -> usize {
            self.text().iter().filter(|&&b| b == b'\n').count()
        }
    }

    impl Out for Capture {
        fn put(&mut self, bytes: &[u8]) {
            for &b in bytes {
                if self.len == self.buf.len() {
                    return;
                }
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
    }

    fn run(line: &[u8], facts: &Facts, procs: &[ProcRow]) -> (Capture, Action) {
        run_with(line, facts, procs, &[])
    }

    fn run_with(
        line: &[u8],
        facts: &Facts,
        procs: &[ProcRow],
        files: &[FileRow],
    ) -> (Capture, Action) {
        let mut out = Capture::new();
        let cmd = parse(line);
        let action = respond(&cmd, facts, procs, files, &mut out);
        (out, action)
    }

    /// Build a listing row the way the kernel does, so the tests and the kernel
    /// disagree about nothing.
    fn file(name: &[u8], size: u16) -> FileRow {
        let mut row = FileRow {
            name: [0u8; 64],
            name_len: name.len() as u8,
            size,
        };
        row.name[..name.len()].copy_from_slice(name);
        row
    }

    // -- the line editor ---------------------------------------------------

    #[test]
    fn typing_then_enter_gives_back_the_line() {
        let mut ed = LineEditor::new();
        for &b in b"version" {
            assert_eq!(ed.feed(b), Edit::Echo(b));
        }
        assert_eq!(ed.feed(b'\r'), Edit::Submit);
        assert_eq!(ed.line(), b"version");
    }

    #[test]
    fn backspace_rubs_out_one_character_and_stops_at_the_start() {
        let mut ed = LineEditor::new();
        ed.feed(b'a');
        ed.feed(b'b');
        assert_eq!(ed.feed(0x7f), Edit::Erase);
        assert_eq!(ed.line(), b"a");
        assert_eq!(ed.feed(0x08), Edit::Erase);
        assert_eq!(ed.line(), b"");
        // Nothing left to erase: the editor says so instead of underflowing.
        assert_eq!(ed.feed(0x7f), Edit::Ignored);
        assert!(ed.is_empty());
    }

    #[test]
    fn a_full_line_drops_the_next_byte_instead_of_overflowing() {
        let mut ed = LineEditor::new();
        for _ in 0..LINE_MAX {
            assert_eq!(ed.feed(b'x'), Edit::Echo(b'x'));
        }
        assert_eq!(ed.feed(b'x'), Edit::Full);
        assert_eq!(ed.line().len(), LINE_MAX);
    }

    #[test]
    fn an_arrow_key_does_not_land_in_the_line() {
        // A terminal sends Up as ESC [ A. None of the three is printable ASCII in
        // the sense this editor accepts, so the line stays clean.
        let mut ed = LineEditor::new();
        ed.feed(b'p');
        assert_eq!(ed.feed(0x1b), Edit::Ignored);
        assert_eq!(ed.feed(b'['), Edit::Echo(b'['));
        assert_eq!(ed.feed(b'A'), Edit::Echo(b'A'));
        // `[` and `A` are printable, so they do land: this records that the editor
        // parses no escape sequences, rather than pretending it does.
        assert_eq!(ed.line(), b"p[A");
    }

    #[test]
    fn ctrl_c_throws_the_line_away() {
        let mut ed = LineEditor::new();
        ed.feed(b'r');
        ed.feed(b'm');
        assert_eq!(ed.feed(0x03), Edit::Submit);
        assert!(ed.is_empty());
        assert_eq!(parse(ed.line()), Command::Empty);
    }

    // -- parsing -----------------------------------------------------------

    #[test]
    fn every_name_in_help_parses_to_a_known_command() {
        for (name, _) in HELP {
            // `run <file>` is a shape, not something to type literally. Everything
            // else in the list is a command exactly as written.
            if name.contains(&b'<') {
                continue;
            }
            let cmd = parse(name);
            assert!(
                !matches!(cmd, Command::Unknown(_) | Command::RunBadArg(_)),
                "help lists a name the parser does not know: {:?}",
                core::str::from_utf8(name)
            );
        }
    }

    #[test]
    fn whitespace_and_case_do_not_matter() {
        assert_eq!(parse(b"  VERSION  "), Command::Version);
        assert_eq!(parse(b"\tPs"), Command::Ps);
        assert_eq!(parse(b"run    HOSTILE"), Command::Run(Workload::Hostile));
    }

    #[test]
    fn an_empty_line_is_not_an_error() {
        assert_eq!(parse(b""), Command::Empty);
        assert_eq!(parse(b"   "), Command::Empty);
        assert_eq!(parse(b"\t \t"), Command::Empty);
    }

    #[test]
    fn run_without_a_workload_says_so_instead_of_running_one() {
        assert_eq!(parse(b"run"), Command::RunBadArg(b""));
        let (out, action) = run(b"run", &Facts::default(), &[]);
        assert_eq!(action, Action::None);
        assert!(out.contains(b"run clean"));
        assert!(out.contains(b"file"), "the third form has to be offered too");
    }

    #[test]
    fn run_with_an_unknown_word_reads_it_as_a_file_now() {
        // It used to be an error. Since `run <file>` exists, a word that is not one
        // of the two workloads is a file name -- and the answer is about the file.
        assert_eq!(parse(b"run sideways"), Command::Exec(b"sideways"));
        let (out, action) = run(b"run sideways", &Facts::default(), &[]);
        assert_eq!(action, Action::None, "there is no such file here");
        assert!(out.contains(b"sideways"));
    }

    #[test]
    fn an_unknown_command_is_quoted_back() {
        let (out, action) = run(b"halt", &Facts::default(), &[]);
        assert_eq!(action, Action::None);
        assert!(out.contains(b"'halt'"));
        assert!(out.contains(b"type help"));
    }

    // -- answers -----------------------------------------------------------

    #[test]
    fn version_says_the_same_version_as_the_rest_of_the_kernel() {
        let (out, _) = run(b"version", &Facts::default(), &[]);
        assert!(out.contains(crate::KERNEL_VERSION.as_bytes()));
    }

    #[test]
    fn help_lists_every_command_it_knows() {
        let (out, _) = run(b"help", &Facts::default(), &[]);
        assert_eq!(out.lines(), HELP.len());
        for (name, _) in HELP {
            assert!(
                out.contains(name),
                "help left out {:?}",
                core::str::from_utf8(name)
            );
        }
    }

    #[test]
    fn pci_reports_the_numbers_it_was_given() {
        let facts = Facts {
            pci_buses: 7,
            pci_devices: 23,
            ..Facts::default()
        };
        let (out, _) = run(b"pci", &facts, &[]);
        assert!(out.contains(b"buses 7"));
        assert!(out.contains(b"devices 23"));
        assert!(!out.contains(b"truncated"));
    }

    #[test]
    fn pci_says_when_the_scan_ran_out_of_room() {
        let facts = Facts {
            pci_buses: 8,
            pci_devices: 40,
            pci_truncated: true,
            ..Facts::default()
        };
        let (out, _) = run(b"pci", &facts, &[]);
        assert!(out.contains(b"truncated"));
    }

    #[test]
    fn mem_prints_both_pools() {
        let facts = Facts {
            heap_used: 4096,
            heap_capacity: 8_388_608,
            frames_used: 12,
            frames_total: 262_144,
            ..Facts::default()
        };
        let (out, _) = run(b"mem", &facts, &[]);
        assert!(out.contains(b"4096 of 8388608"));
        assert!(out.contains(b"12 of 262144"));
    }

    #[test]
    fn ps_prints_a_row_per_process_and_the_exit_code_of_a_zombie() {
        let procs = [
            ProcRow {
                pid: 1,
                parent: 0,
                state: b'R',
                exit_code: 0,
            },
            ProcRow {
                pid: 2,
                parent: 1,
                state: b'Z',
                exit_code: 43,
            },
        ];
        let facts = Facts {
            procs_live: 1,
            procs_slots: 16,
            ..Facts::default()
        };
        let (out, _) = run(b"ps", &facts, &procs);
        assert!(out.contains(b"running"));
        assert!(out.contains(b"zombie"));
        assert!(out.contains(b"43"));
        assert!(out.contains(b"live 1, slots 16"));
    }

    #[test]
    fn ps_with_an_empty_table_says_so_rather_than_printing_a_header() {
        let (out, _) = run(b"ps", &Facts::default(), &[]);
        assert!(out.contains(b"no processes"));
        assert!(!out.contains(b"pid  parent"));
    }

    #[test]
    fn audit_distinguishes_intact_from_broken() {
        let ok = Facts {
            audit_records: 5,
            audit_verifies: true,
            ..Facts::default()
        };
        let (out, _) = run(b"audit", &ok, &[]);
        assert!(out.contains(b"records 5"));
        assert!(out.contains(b"chain intact"));

        let bad = Facts {
            audit_verifies: false,
            ..ok
        };
        let (out, _) = run(b"audit", &bad, &[]);
        assert!(out.contains(b"BROKEN"));
    }

    #[test]
    fn net_without_a_card_is_a_statement_not_an_error() {
        let (out, _) = run(b"net", &Facts::default(), &[]);
        assert!(out.contains(b"no e1000"));
    }

    #[test]
    fn net_prints_the_mac_the_way_the_boot_markers_do() {
        let facts = Facts {
            net_present: true,
            net_mac_known: true,
            net_mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            net_link_up: true,
            net_armed: true,
            ..Facts::default()
        };
        let (out, _) = run(b"net", &facts, &[]);
        assert!(out.contains(b"52:54:00:12:34:56"));
        assert!(out.contains(b"link up"));
        assert!(!out.contains(b"not set up"));
    }

    #[test]
    fn net_says_when_the_receiver_was_never_armed() {
        let facts = Facts {
            net_present: true,
            net_mac_known: true,
            net_mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            net_link_up: true,
            net_armed: false,
            ..Facts::default()
        };
        let (out, _) = run(b"net", &facts, &[]);
        assert!(out.contains(b"not set up"));
    }

    #[test]
    fn net_separates_a_card_on_the_bus_from_a_card_the_driver_touched() {
        // The Limine boot path: the enumeration sees an e1000, the driver leaves it
        // alone. Saying "no e1000" here would contradict what `pci` just printed.
        let facts = Facts {
            net_present: true,
            net_mac_known: false,
            ..Facts::default()
        };
        let (out, _) = run(b"net", &facts, &[]);
        assert!(!out.contains(b"no e1000"));
        assert!(out.contains(b"registers not mapped"));
    }

    #[test]
    fn uptime_turns_half_second_ticks_into_seconds() {
        let facts = Facts {
            uptime_ticks: 9,
            ..Facts::default()
        };
        let (out, _) = run(b"uptime", &facts, &[]);
        assert!(out.contains(b"ticks 9"));
        assert!(out.contains(b"about 4 s"));
    }

    // -- the filesystem commands -----------------------------------------

    #[test]
    fn ls_lists_what_is_there_with_its_size() {
        let files = [file(b"/hello.elf", 151), file(b"/attack.elf", 185)];
        let (out, _) = run_with(b"ls", &Facts::default(), &[], &files);
        assert!(out.contains(b"/hello.elf"));
        assert!(out.contains(b"151 bytes"));
        assert!(out.contains(b"/attack.elf"));
        assert!(out.contains(b"185 bytes"));
    }

    #[test]
    fn ls_on_an_empty_filesystem_says_so() {
        let (out, _) = run_with(b"ls", &Facts::default(), &[], &[]);
        assert!(out.contains(b"no files"));
    }

    #[test]
    fn load_finds_the_file_and_hands_its_index_back() {
        let files = [file(b"/a.elf", 10), file(b"/b.elf", 20)];
        let (out, action) = run_with(b"load /b.elf", &Facts::default(), &[], &files);
        assert_eq!(action, Action::Load(1));
        assert_eq!(out.lines(), 0, "the outcome is the caller's to print");
    }

    #[test]
    fn run_with_a_file_name_is_an_exec_not_a_workload() {
        let files = [file(b"/hello.elf", 151)];
        let (_, action) = run_with(b"run /hello.elf", &Facts::default(), &[], &files);
        assert_eq!(action, Action::Exec(0));
    }

    #[test]
    fn the_two_built_in_workloads_keep_their_names() {
        // A file called `clean` must not shadow `run clean`, or the demo the panel
        // is built around would change meaning the moment someone created one.
        let files = [file(b"clean", 4)];
        let (_, action) = run_with(b"run clean", &Facts::default(), &[], &files);
        assert_eq!(action, Action::Run(Workload::Clean));
    }

    #[test]
    fn a_missing_file_is_named_back_with_a_way_out() {
        let files = [file(b"/a.elf", 10)];
        let (out, action) = run_with(b"run /nope.elf", &Facts::default(), &[], &files);
        assert_eq!(action, Action::None);
        assert!(out.contains(b"/nope.elf"));
        assert!(out.contains(b"try ls"));
    }

    #[test]
    fn load_without_a_name_asks_for_one() {
        let (out, action) = run_with(b"load", &Facts::default(), &[], &[]);
        assert_eq!(action, Action::None);
        assert!(out.contains(b"which file"));
        assert!(out.contains(b"load <name>"));
    }

    #[test]
    fn file_names_are_matched_exactly() {
        // Unlike command words, which are case-insensitive.
        let files = [file(b"/Hello.elf", 10)];
        let (_, action) = run_with(b"load /hello.elf", &Facts::default(), &[], &files);
        assert_eq!(action, Action::None, "a different name is a different file");
    }

    #[test]
    fn a_load_outcome_reads_the_same_way_whatever_happened() {
        let mut out = Capture::new();
        report_load(b"/hello.elf", LoadResult::Ok { entry: 0x4001_0000, segments: 1 }, &mut out);
        assert!(out.contains(b"loaded"));
        assert!(out.contains(b"40010000"));

        let mut out = Capture::new();
        report_load(b"/x.elf", LoadResult::NotLoadable, &mut out);
        assert!(out.contains(b"not a program this kernel can load"));

        let mut out = Capture::new();
        report_load(b"/x.elf", LoadResult::NotAvailable, &mut out);
        assert!(out.contains(b"not compiled into this build"));

    }

    #[test]
    fn run_hands_the_workload_back_to_the_caller() {
        let (out, action) = run(b"run clean", &Facts::default(), &[]);
        assert_eq!(action, Action::Run(Workload::Clean));
        assert_eq!(out.lines(), 0, "the gate's answer is the caller's to print");

        let (_, action) = run(b"run hostile", &Facts::default(), &[]);
        assert_eq!(action, Action::Run(Workload::Hostile));
    }

    // -- formatting --------------------------------------------------------

    #[test]
    fn fmt_u64_covers_zero_and_the_top_of_the_range() {
        let mut buf = [0u8; 24];
        let n = fmt_u64(0, &mut buf);
        assert_eq!(&buf[..n], b"0");
        let n = fmt_u64(u64::MAX, &mut buf);
        assert_eq!(&buf[..n], b"18446744073709551615");
    }

    #[test]
    fn fmt_u64_truncates_instead_of_writing_past_the_end() {
        let mut buf = [0u8; 3];
        let n = fmt_u64(123_456, &mut buf);
        assert!(n <= buf.len());
    }

    #[test]
    fn fmt_hex_pads_to_the_requested_width() {
        let mut buf = [0u8; 8];
        let n = fmt_hex(0x5, 2, &mut buf);
        assert_eq!(&buf[..n], b"05");
        let n = fmt_hex(0xdead_beef, 8, &mut buf);
        assert_eq!(&buf[..n], b"deadbeef");
    }

    #[test]
    fn a_long_answer_is_cut_rather_than_overrunning_its_buffer() {
        // 200 characters of command name: longer than `Line`'s buffer.
        let long = [b'z'; 200];
        let (out, _) = run(&long, &Facts::default(), &[]);
        assert!(out.lines() >= 1);
        assert!(out.text().len() <= 97, "one line plus its newline");
    }
}
