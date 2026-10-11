//! Hardware diagnostic image: one screen that says what a machine is doing at boot.
//!
//! Built only with the `hw_diag` feature, for a test ISO that never enters the kernel.
//! It exists for two machines the kernel does not work on:
//!
//! - a 2009 laptop booting legacy BIOS from USB, where the screen comes up and the USB
//!   keyboard (reaching the kernel, if at all, through the firmware's PS/2 emulation)
//!   does nothing;
//! - a UEFI mini PC where the boot ends back in the installed system, which is what a
//!   reset looks like as much as it is what a skipped USB stick looks like.
//!
//! A photograph of the screen has to answer both. So every step is drawn, numbered,
//! *before* it runs, and its result is added to the same line after: if a step hangs or
//! faults, the last line on screen names it. Faults land in an exception catcher that
//! prints the vector and the address instead of resetting the machine.
//!
//! **Read-only by intent.** No disk is touched, no USB controller is reset or claimed,
//! no device is programmed. The writes it does make: the COM1 setup, the CMOS index
//! port, the PCI address latch (which selects a register), and the two i8042 commands it
//! is asked to send -- read the configuration byte, run the self-test -- plus putting the
//! configuration byte back if the self-test changed it. Those commands come last, after
//! a window where nothing at all is sent to the controller, so whatever they do cannot
//! colour the first reading.
//!
//! Everything on the screen also goes to COM1, prefixed `[DIAG]`.

pub mod decode;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod cpu;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod probes;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod screen;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub use bare::*;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod bare {
    use core::fmt::{self, Write};
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    pub use super::screen::Color;

    use super::cpu;
    use super::decode::{self, ByteRing, Line, MemSummary, ST_AUX, ST_OUTPUT_FULL};
    use super::probes::{self, CmdError, Legacy, UsbController};
    use super::screen;

    /// Rows at the bottom of the screen that belong to the live readout.
    const LIVE_ROWS: usize = 6;
    /// First row stage lines go on; row 0 is the title.
    const FIRST_STAGE_ROW: usize = 1;
    /// How long each listening window before the last one lasts.
    const WINDOW_MS: u64 = 10_000;
    /// Pause after drawing each stage line, before running the step, so a machine that
    /// resets still shows the step for long enough to be seen (or filmed).
    const STAGE_PAUSE_MS: u64 = 250;

    static STAGE: AtomicU32 = AtomicU32::new(0);
    static NEXT_ROW: AtomicUsize = AtomicUsize::new(FIRST_STAGE_ROW);
    /// Where the current stage line's text ends, so its result can be appended.
    static CUR_ROW: AtomicUsize = AtomicUsize::new(FIRST_STAGE_ROW);
    static CUR_COL: AtomicUsize = AtomicUsize::new(0);
    /// The current stage line, repeated in front of its result on COM1 so the serial
    /// log reads one line per step.
    static mut CUR_TITLE: Line = Line::new();

    fn cur_title() -> &'static mut Line {
        // SAFETY: one CPU, interrupts off; only `stage` and `result` touch it, never at
        // the same time.
        unsafe { &mut *core::ptr::addr_of_mut!(CUR_TITLE) }
    }

    fn serial_line(text: &[u8]) {
        cpu::com1_write(b"[DIAG] ");
        cpu::com1_write(text);
        cpu::com1_write(b"\r\n");
    }

    /// Last row stage lines may use.
    fn stage_limit() -> usize {
        screen::rows().saturating_sub(LIVE_ROWS + 1)
    }

    /// Take a row for a new line, or `None` once the stage area is full (the line then
    /// goes to COM1 only, and the last row says so).
    fn take_row() -> Option<usize> {
        let row = NEXT_ROW.load(Ordering::Relaxed);
        let limit = stage_limit();
        if row > limit {
            return None;
        }
        NEXT_ROW.store(row + 1, Ordering::Relaxed);
        if row == limit {
            screen::put(row, 0, b"     (more lines on COM1)", Color::Grey);
            return None;
        }
        Some(row)
    }

    /// Start a numbered step: draw it, send it to COM1, and pause briefly.
    pub fn stage(title: &str) {
        let n = STAGE.fetch_add(1, Ordering::Relaxed) + 1;
        let mut line = Line::new();
        let _ = write!(line, "[{n:02}] {title}");
        serial_line(line.as_bytes());
        *cur_title() = line;
        match take_row() {
            Some(row) => {
                screen::put(row, 0, line.as_bytes(), Color::White);
                CUR_ROW.store(row, Ordering::Relaxed);
                CUR_COL.store(line.len(), Ordering::Relaxed);
            }
            None => CUR_COL.store(usize::MAX, Ordering::Relaxed),
        }
        screen::flush();
        cpu::delay_ms(STAGE_PAUSE_MS);
    }

    /// Finish the current step: append ` -> result` to its line.
    pub fn result(color: Color, args: fmt::Arguments) {
        let mut line = Line::new();
        let _ = line.write_str(" -> ");
        let _ = line.write_fmt(args);
        let mut serial = *cur_title();
        serial.push_bytes(line.as_bytes());
        serial_line(serial.as_bytes());
        let col = CUR_COL.load(Ordering::Relaxed);
        if col != usize::MAX {
            let row = CUR_ROW.load(Ordering::Relaxed);
            screen::clear_from(row, col);
            screen::put(row, col, line.as_bytes(), color);
        }
        screen::flush();
    }

    /// An extra, indented line under the current step.
    pub fn detail(color: Color, args: fmt::Arguments) {
        let mut line = Line::new();
        let _ = line.write_str("     ");
        let _ = line.write_fmt(args);
        serial_line(line.as_bytes());
        if let Some(row) = take_row() {
            screen::put(row, 0, line.as_bytes(), color);
        }
        screen::flush();
    }

    /// COM1 only: detail too long or too plentiful for the screen.
    pub fn serial(args: fmt::Arguments) {
        let mut line = Line::new();
        let _ = line.write_fmt(args);
        serial_line(line.as_bytes());
    }

    /// The first two steps, which need no bootloader answer: COM1 and the exception
    /// catcher. They are drawn now and appear when the framebuffer does.
    pub fn early_init() {
        cpu::clock_start();
        let mut title = Line::new();
        let _ = write!(
            title,
            "ZPL HW DIAG  kernel {}  read-only: no disk, no USB reset",
            crate::KERNEL_VERSION
        );
        screen::put(0, 0, title.as_bytes(), Color::Cyan);
        cpu::com1_write(b"\r\n");
        serial_line(title.as_bytes());

        stage("COM1 serial 115200 8N1");
        cpu::com1_init();
        result(Color::Green, format_args!("ok"));

        stage("exception catcher (IDT, 32 vectors)");
        cpu::install_idt();
        result(Color::Green, format_args!("ok"));
    }

    /// Tell the diagnostic where the direct map is, so it can check addresses.
    pub fn set_hhdm(offset: u64) {
        cpu::set_hhdm(offset);
    }

    /// Is `[va, va + len)` mapped right now?
    #[must_use]
    pub fn mapped(va: u64, len: u64) -> bool {
        cpu::range_mapped(va, len)
    }

    /// Draw on the framebuffer from now on. Reports the result on the current stage.
    #[allow(clippy::too_many_arguments)]
    pub fn attach_framebuffer(
        va: u64,
        width: u64,
        height: u64,
        pitch: u64,
        bpp: u16,
        r_shift: u8,
        g_shift: u8,
        b_shift: u8,
    ) {
        match screen::attach(va, width, height, pitch, bpp, r_shift, g_shift, b_shift) {
            Ok((scale, cols, rows)) => result(
                Color::Green,
                format_args!("{width}x{height} {bpp}bpp, text x{scale} ({cols}x{rows})"),
            ),
            Err(screen::AttachError::Depth(d)) => result(
                Color::Red,
                format_args!("{width}x{height} at {d}bpp: cannot draw that depth"),
            ),
            Err(screen::AttachError::TooSmall) => {
                result(Color::Red, format_args!("{width}x{height}: too small to draw on"));
            }
        }
    }

    /// Summarise the memory map on the current stage, every entry on COM1.
    pub fn report_memmap(entries: &mut dyn Iterator<Item = (u64, u64, u64)>) {
        let mut m = MemSummary::default();
        for (base, len, kind) in entries {
            serial(format_args!("  mem {base:#014x} +{len:#014x} type {kind}"));
            m.add(base, len, kind);
        }
        result(
            if m.usable_bytes > 0 { Color::Green } else { Color::Red },
            format_args!(
                "{} entries, {} MiB usable in {} ranges, top {:#x}",
                m.entries,
                m.usable_bytes >> 20,
                m.usable_ranges,
                m.top_usable
            ),
        );
        let mut line = Line::new();
        for (name, count) in decode::MEMMAP_TYPE_NAMES.iter().zip(m.per_type).skip(1) {
            if count > 0 {
                let _ = write!(line, "{name} {count}  ");
            }
        }
        if m.other > 0 {
            let _ = write!(line, "other {}", m.other);
        }
        detail(Color::Grey, format_args!("{}", Str(line.as_bytes())));
    }

    /// Printable bytes as a `Display`, for passing a `Line` through `format_args!`.
    struct Str<'a>(&'a [u8]);

    impl fmt::Display for Str<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(core::str::from_utf8(self.0).unwrap_or("?"))
        }
    }

    /// Called from the exception stubs. Draws the fault where the next line would go,
    /// in red, and on COM1; the caller then halts.
    pub(super) fn report_exception(frame: &cpu::ExceptionFrame) {
        let stage = STAGE.load(Ordering::Relaxed);
        let mut a = Line::new();
        let _ = write!(
            a,
            "!! CPU EXCEPTION {} {} during step {stage:02}",
            frame.vector,
            decode::exception_name(frame.vector)
        );
        let mut b = Line::new();
        let _ = write!(
            b,
            "!! rip={:#x} err={:#x} cr2={:#x} rsp={:#x}",
            frame.rip,
            frame.error,
            cpu::read_cr2(),
            frame.rsp
        );
        for line in [&a, &b] {
            serial_line(line.as_bytes());
            let row = NEXT_ROW.load(Ordering::Relaxed).min(stage_limit());
            NEXT_ROW.store(row + 1, Ordering::Relaxed);
            screen::clear_from(row, 0);
            screen::put(row, 0, line.as_bytes(), Color::Red);
        }
        screen::flush();
    }

    // -----------------------------------------------------------------------
    // The hardware steps, and the live readout
    // -----------------------------------------------------------------------

    /// Bytes read from 0x60, per window, split keyboard / auxiliary.
    struct Live {
        /// `[window][aux]`; window 0 is everything outside the three listening windows
        /// (the drain at boot, bytes that arrived during a command), 1..=3 are A, B, C.
        counts: [[u32; 2]; 4],
        kbd: ByteRing,
        aux: ByteRing,
        polls: u64,
        status: u8,
        logged: u32,
    }

    static mut LIVE: Live = Live {
        counts: [[0; 2]; 4],
        kbd: ByteRing::new(),
        aux: ByteRing::new(),
        polls: 0,
        status: 0,
        logged: 0,
    };

    fn live() -> &'static mut Live {
        // SAFETY: one CPU, interrupts off, and only this module's straight-line code
        // touches it; the exception path does not.
        unsafe { &mut *core::ptr::addr_of_mut!(LIVE) }
    }

    const WINDOW_NAMES: [&str; 4] = ["other", "A", "B", "C"];

    fn record(window: usize, byte: u8, status: u8) {
        let l = live();
        let aux = status & ST_AUX != 0;
        l.counts[window][usize::from(aux)] += 1;
        if aux {
            l.aux.push(byte);
        } else {
            l.kbd.push(byte);
        }
        // Every byte on COM1 up to a limit, so a controller stuck on "full" cannot
        // drown the log.
        if l.logged < 400 {
            l.logged += 1;
            serial(format_args!(
                "  0x60 window {} {} byte {byte:#04x} status {status:#04x}",
                WINDOW_NAMES[window],
                if aux { "aux" } else { "kbd" }
            ));
        }
    }

    /// Run everything after the bootloader's answers, then listen forever.
    pub fn run_probes(rsdp: Option<u64>) -> ! {
        stage("clock: TSC against CMOS RTC");
        match cpu::calibrate() {
            Some(mhz) => result(Color::Green, format_args!("TSC {mhz} MHz")),
            None => result(Color::Yellow, format_args!("RTC not ticking; assuming 2000 MHz")),
        }

        step_acpi(rsdp);
        step_i8042_passive();

        stage("window A: listen 10 s, nothing sent");
        listen(1, Some(WINDOW_MS));
        window_result(1);

        step_pci();

        stage("window B: listen 10 s after PCI scan");
        listen(2, Some(WINDOW_MS));
        window_result(2);

        step_i8042_commands();

        stage("window C: listen forever; type, then photo");
        result(Color::Cyan, format_args!("running"));
        listen(3, None);
        cpu::halt_forever()
    }

    fn step_acpi(rsdp: Option<u64>) {
        stage("ACPI RSDP and root table");
        let (Some(addr), Some(hhdm)) = (rsdp, cpu::hhdm()) else {
            result(Color::Yellow, format_args!("no RSDP from Limine; skipped"));
            stage("ACPI FADT 8042 flag");
            result(Color::Yellow, format_args!("skipped"));
            return;
        };
        let r = probes::read_acpi(addr, hhdm);
        match (r.rsdp, r.root) {
            (Some(p), Some((root, wide))) => result(
                if r.problem.is_some() { Color::Yellow } else { Color::Green },
                format_args!(
                    "rev {} {} {:#x}, {} tables{}{}",
                    p.revision,
                    if wide { "XSDT" } else { "RSDT" },
                    root,
                    r.table_count,
                    if p.checksum_ok { "" } else { ", BAD CHECKSUM" },
                    if r.problem.is_some() { " (incomplete)" } else { "" }
                ),
            ),
            _ => result(Color::Red, format_args!("{}", r.problem.unwrap_or("unreadable"))),
        }
        if let Some(p) = r.problem {
            serial(format_args!("  ACPI problem: {p}"));
        }
        let mut sigs = Line::new();
        for s in r.signatures.iter().take(r.table_count) {
            sigs.push_bytes(s);
            sigs.push_bytes(b" ");
        }
        serial(format_args!("  ACPI tables: {}", Str(sigs.as_bytes())));

        stage("ACPI FADT 8042 flag");
        match r.fadt {
            Some(f) => match f.boot_arch {
                Some(v) if f.boot_arch_defined() => {
                    let mut bits = Line::new();
                    decode::describe_boot_arch(v, &mut bits);
                    let present = v & decode::BOOT_ARCH_8042 != 0;
                    result(
                        if present { Color::Green } else { Color::Yellow },
                        format_args!(
                            "8042={} (boot_arch {v:#06x}, FADT rev {})",
                            if present { "YES" } else { "NO" },
                            f.revision
                        ),
                    );
                    serial(format_args!("  {}", Str(bits.as_bytes())));
                }
                // An ACPI 1.0 table: the two bytes are reserved there and mean nothing,
                // so no answer is printed rather than a "NO" that would be read as one.
                Some(v) => result(
                    Color::Grey,
                    format_args!("8042=n/a (FADT rev {} predates the flag; raw {v:#06x})", f.revision),
                ),
                None => result(
                    Color::Yellow,
                    format_args!("FADT rev {} len {}: too short for the flag", f.revision, f.length),
                ),
            },
            None => result(Color::Yellow, format_args!("no FADT found")),
        }
    }

    fn step_i8042_passive() {
        stage("i8042 status 0x64");
        let s = probes::status();
        let mut desc = Line::new();
        decode::describe_i8042_status(s, &mut desc);
        result(
            if s == 0xFF { Color::Red } else { Color::Green },
            format_args!("{s:#04x} {}", Str(desc.as_bytes())),
        );

        stage("i8042 drain 0x60");
        if s == 0xFF {
            result(Color::Yellow, format_args!("skipped, no controller"));
            return;
        }
        let mut got = Line::new();
        let mut n = 0;
        for _ in 0..64 {
            let st = probes::status();
            if st & ST_OUTPUT_FULL == 0 {
                break;
            }
            let b = probes::data();
            record(0, b, st);
            let _ = write!(got, "{}{b:02x} ", if st & ST_AUX != 0 { "a:" } else { "" });
            n += 1;
        }
        if n == 0 {
            result(Color::Green, format_args!("empty"));
        } else {
            result(Color::Yellow, format_args!("{n} waiting: {}", Str(got.as_bytes())));
        }
    }

    fn step_pci() {
        stage("PCI scan: USB controllers");
        let r = probes::scan_pci();
        result(
            if r.usb_total > 0 { Color::Green } else { Color::Yellow },
            format_args!("{} devices, {} USB controllers", r.devices, r.usb_total),
        );
        for c in r.usb.iter().flatten() {
            usb_line(c);
        }
        if r.usb_total > r.usb.len() {
            detail(Color::Grey, format_args!("... {} more", r.usb_total - r.usb.len()));
        }
    }

    fn usb_line(c: &UsbController) {
        let mut l = Line::new();
        let _ = write!(
            l,
            "{} {:02x}:{:02x}.{} {:04x}:{:04x} irq{} ",
            decode::usb_kind(c.prog_if),
            c.bus,
            c.dev,
            c.func,
            c.vendor,
            c.device,
            c.irq
        );
        let color = match c.legacy {
            Legacy::Uhci(v) => {
                decode::describe_uhci_legsup(v, &mut l);
                if v & 0x10 != 0 { Color::Yellow } else { Color::White }
            }
            Legacy::LegSup { legsup, ctlsts, guessed } => {
                decode::describe_usb_legsup(legsup, ctlsts, &mut l);
                if guessed {
                    l.push_bytes(b" @cfg68");
                }
                if legsup & (1 << 16) != 0 { Color::Yellow } else { Color::White }
            }
            Legacy::Ohci(ctl) => {
                let _ = write!(l, "hccontrol={ctl:#x} smm_owns={}", u8::from(ctl & 0x100 != 0));
                if ctl & 0x100 != 0 { Color::Yellow } else { Color::White }
            }
            Legacy::None => {
                l.push_bytes(b"no legacy register");
                Color::White
            }
            Legacy::Unmapped => {
                l.push_bytes(b"registers not mapped");
                Color::Grey
            }
        };
        detail(color, format_args!("{}", Str(l.as_bytes())));
    }

    fn step_i8042_commands() {
        let aux_sink = &mut |b: u8| record(0, b, ST_AUX);
        stage("8042 cmd 0x20 config");
        if probes::status() == 0xFF {
            result(Color::Yellow, format_args!("skipped, no controller"));
            stage("8042 cmd 0xAA self-test");
            result(Color::Yellow, format_args!("skipped, no controller"));
            return;
        }
        let before = match probes::command_read(0x20, 200, aux_sink) {
            Ok(c) => {
                let mut d = Line::new();
                decode::describe_i8042_config(c, &mut d);
                let off = c & 0x10 != 0;
                result(
                    if off { Color::Red } else { Color::Green },
                    format_args!("{c:#04x} {}", Str(d.as_bytes())),
                );
                Some(c)
            }
            Err(e) => {
                result(Color::Red, format_args!("{}", cmd_error(e)));
                None
            }
        };

        stage("8042 cmd 0xAA self-test");
        match probes::command_read(0xAA, 1000, aux_sink) {
            Ok(0x55) => result(Color::Green, format_args!("0x55 passed")),
            Ok(other) => result(Color::Red, format_args!("{other:#04x} (0x55 = pass)")),
            Err(e) => result(Color::Red, format_args!("{}", cmd_error(e))),
        }
        let Some(before) = before else { return };
        match probes::command_read(0x20, 200, aux_sink) {
            Ok(after) if after == before => {
                detail(Color::Grey, format_args!("config after self-test {after:#04x}: unchanged"));
            }
            Ok(after) => {
                let restored = probes::write_config(before)
                    && probes::command_read(0x20, 200, aux_sink) == Ok(before);
                detail(
                    Color::Yellow,
                    format_args!(
                        "config after self-test {after:#04x}: changed; restore {before:#04x} {}",
                        if restored { "ok" } else { "FAILED" }
                    ),
                );
            }
            Err(e) => detail(Color::Red, format_args!("config after self-test: {}", cmd_error(e))),
        }
    }

    fn cmd_error(e: CmdError) -> &'static str {
        match e {
            CmdError::NotAccepted => "not accepted (input buffer stayed full)",
            CmdError::NoReply => "no reply (timeout)",
        }
    }

    fn window_result(window: usize) {
        let [kbd, aux] = live().counts[window];
        result(
            if kbd > 0 { Color::Green } else { Color::Red },
            format_args!("kbd {kbd}, aux {aux} bytes"),
        );
    }

    /// Poll 0x64 / 0x60 and keep the live rows current. `None` means forever.
    fn listen(window: usize, duration_ms: Option<u64>) {
        let start = cpu::uptime_ms();
        let mut last_draw = 0u64;
        loop {
            let l = live();
            let s = probes::status();
            l.status = s;
            l.polls += 1;
            let mut changed = false;
            // 0xFF is an absent controller, not a full one; reading 0x60 then would only
            // count the bus floating.
            if s != 0xFF && s & ST_OUTPUT_FULL != 0 {
                let b = probes::data();
                record(window, b, s);
                changed = true;
            }
            let now = cpu::uptime_ms();
            let elapsed = now.saturating_sub(start);
            let done = duration_ms.is_some_and(|d| elapsed >= d);
            if changed || done || now.saturating_sub(last_draw) >= 200 {
                draw_live(window, duration_ms.map(|d| d.saturating_sub(elapsed)));
                last_draw = now;
            }
            if done {
                return;
            }
        }
    }

    fn draw_live(window: usize, remaining_ms: Option<u64>) {
        let l = live();
        let top = screen::rows().saturating_sub(LIVE_ROWS);
        let (h, m, s) = cpu::rtc_time();
        let mut rows: [Line; LIVE_ROWS] = [Line::new(); LIVE_ROWS];

        let _ = write!(
            rows[0],
            "LIVE  up {:>4}s  rtc {h:02}:{m:02}:{s:02}  polls {:>7}k  window {}",
            cpu::uptime_ms() / 1000,
            l.polls / 1000,
            WINDOW_NAMES[window]
        );
        if let Some(ms) = remaining_ms {
            let _ = write!(rows[0], " ({}s left)", ms.div_ceil(1000));
        }
        let total: u32 = l.counts.iter().map(|c| c[0] + c[1]).sum();
        let _ = write!(
            rows[1],
            "0x64 now {:#04x}   bytes read from 0x60: {total}  (kbd {}, aux {})",
            l.status,
            l.kbd.total(),
            l.aux.total()
        );
        for w in 1..=3 {
            let [k, a] = l.counts[w];
            let _ = write!(rows[2], "{}: kbd {k:<4} aux {a:<4}  ", WINDOW_NAMES[w]);
        }
        let _ = write!(rows[2], "other: {}", l.counts[0][0] + l.counts[0][1]);
        let _ = rows[3].write_str("last kbd: ");
        l.kbd.write_hex(&mut rows[3]);
        let _ = rows[4].write_str("last aux: ");
        l.aux.write_hex(&mut rows[4]);
        let _ = rows[5].write_str(match window {
            1 => ">> PRESS KEYS NOW on the USB keyboard (window A) <<",
            2 => ">> KEEP PRESSING KEYS (window B) <<",
            _ => ">> press keys a few times, then take the photo <<",
        });

        let colors = [Color::Cyan, Color::White, Color::White, Color::Green, Color::Grey, Color::Yellow];
        for (i, (line, color)) in rows.iter().zip(colors).enumerate() {
            screen::clear_from(top + i, 0);
            screen::put(top + i, 0, line.as_bytes(), color);
        }
        screen::flush();
    }
}
