//! The half of the hardware diagnostic that is plain data handling.
//!
//! Everything here turns bytes the hardware or the bootloader handed over into text a
//! person can read off a photograph. None of it touches a port or an address, so it is
//! compiled and tested on the host like any other code; the bare-metal half in the
//! sibling modules only fetches the bytes.
//!
//! The rule for every decoder: print the raw value as well as the reading of it. A
//! decoder can be wrong, and a photograph with only the interpretation on it cannot be
//! re-read later.

use core::fmt;

/// A fixed line of text, filled with `write!` and never allocating.
///
/// Anything past the capacity is dropped silently: a line that is cut short on screen
/// is still a line, and a diagnostic must not be the thing that panics.
#[derive(Clone, Copy)]
pub struct Line {
    buf: [u8; Self::CAP],
    len: usize,
}

impl Line {
    pub const CAP: usize = 160;

    #[must_use]
    pub const fn new() -> Self {
        Self { buf: [0; Self::CAP], len: 0 }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.len == Self::CAP {
                return;
            }
            // Anything the 8x8 font cannot draw becomes a dot, so a bootloader string
            // with odd bytes in it cannot leave holes in a line.
            self.buf[self.len] = if (0x20..=0x7E).contains(&b) { b } else { b'.' };
            self.len += 1;
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_bytes(s.as_bytes());
        Ok(())
    }
}

/// Text scale for the screen: as large as still leaves an 80 x 36 grid.
///
/// 80 columns by 36 rows is what the diagnostic's layout needs. On the 1366x768 panel of
/// a 2009 laptop that gives 2x (16x20 pixel cells), on 1920x1080 3x, and anything small
/// falls back to 1x rather than not drawing.
#[must_use]
pub fn pick_scale(width: u64, height: u64) -> u32 {
    let by_w = width / (8 * 80);
    let by_h = height / (10 * 36);
    by_w.min(by_h).clamp(1, 4) as u32
}

/// Limine's firmware type, as a word.
#[must_use]
pub fn firmware_name(kind: u64) -> &'static str {
    match kind {
        0 => "BIOS (legacy)",
        1 => "UEFI 32-bit",
        2 => "UEFI 64-bit",
        3 => "SBI",
        _ => "unknown",
    }
}

/// The memory map, boiled down to what fits on two lines.
#[derive(Clone, Copy, Default)]
pub struct MemSummary {
    pub entries: u32,
    pub usable_ranges: u32,
    pub usable_bytes: u64,
    /// One past the last usable byte.
    pub top_usable: u64,
    /// Entries per Limine type, `0..=8`; anything else counts in `other`.
    pub per_type: [u32; 9],
    pub other: u32,
}

impl MemSummary {
    pub fn add(&mut self, base: u64, length: u64, kind: u64) {
        self.entries += 1;
        match usize::try_from(kind).ok().and_then(|k| self.per_type.get_mut(k)) {
            Some(slot) => *slot += 1,
            None => self.other += 1,
        }
        if kind == MEMMAP_USABLE && length > 0 {
            self.usable_ranges += 1;
            self.usable_bytes = self.usable_bytes.saturating_add(length);
            self.top_usable = self.top_usable.max(base.saturating_add(length));
        }
    }
}

/// Limine memory map type for ordinary free RAM.
pub const MEMMAP_USABLE: u64 = 0;

/// Short names for Limine's memory map types, in order.
pub const MEMMAP_TYPE_NAMES: [&str; 9] = [
    "usable", "rsvd", "acpi-recl", "acpi-nvs", "bad", "boot-recl", "kernel", "fb", "map-rsvd",
];

// ---------------------------------------------------------------------------
// ACPI
// ---------------------------------------------------------------------------

/// Sum of all bytes is zero modulo 256: the ACPI checksum rule.
#[must_use]
pub fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0
}

/// What the Root System Description Pointer says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt: u32,
    /// Zero on ACPI 1.0, where the field does not exist.
    pub xsdt: u64,
    /// Checksum over the first 20 bytes (the ACPI 1.0 part).
    pub checksum_ok: bool,
}

/// Bytes of an RSDP worth reading: the ACPI 2.0 structure is 36 long.
pub const RSDP_LEN: usize = 36;

/// Parse an RSDP. `None` unless the signature is there.
///
/// `bytes` may be 20 long (ACPI 1.0) or longer; the 64-bit table pointer is only read
/// when the revision says it exists and the slice holds it.
#[must_use]
pub fn parse_rsdp(bytes: &[u8]) -> Option<Rsdp> {
    if bytes.len() < 20 || &bytes[..8] != b"RSD PTR " {
        return None;
    }
    let revision = bytes[15];
    let rsdt = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let xsdt = if revision >= 2 && bytes.len() >= 32 {
        let mut x = [0u8; 8];
        x.copy_from_slice(&bytes[24..32]);
        u64::from_le_bytes(x)
    } else {
        0
    };
    Some(Rsdp { revision, rsdt, xsdt, checksum_ok: checksum_ok(&bytes[..20]) })
}

/// The common 36-byte header every ACPI table starts with.
pub const SDT_HEADER_LEN: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
}

#[must_use]
pub fn parse_sdt_header(bytes: &[u8]) -> Option<SdtHeader> {
    if bytes.len() < SDT_HEADER_LEN {
        return None;
    }
    let mut signature = [0u8; 4];
    signature.copy_from_slice(&bytes[..4]);
    Some(SdtHeader {
        signature,
        length: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        revision: bytes[8],
    })
}

/// Offset of IAPC_BOOT_ARCH in the FADT.
pub const FADT_BOOT_ARCH_OFFSET: usize = 109;

/// What the FADT says about legacy devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fadt {
    pub revision: u8,
    pub length: u32,
    /// IAPC_BOOT_ARCH, when the table is long enough to carry it.
    pub boot_arch: Option<u16>,
}

impl Fadt {
    /// The field was introduced with ACPI 2.0 (FADT revision 3). In an older table the
    /// same two bytes are reserved and may hold anything; Linux applies the same rule.
    #[must_use]
    pub fn boot_arch_defined(&self) -> bool {
        self.revision >= 3 && self.boot_arch.is_some()
    }
}

#[must_use]
pub fn parse_fadt(bytes: &[u8]) -> Option<Fadt> {
    let header = parse_sdt_header(bytes)?;
    if &header.signature != b"FACP" {
        return None;
    }
    let end = FADT_BOOT_ARCH_OFFSET + 2;
    let boot_arch = if header.length as usize >= end && bytes.len() >= end {
        Some(u16::from_le_bytes([
            bytes[FADT_BOOT_ARCH_OFFSET],
            bytes[FADT_BOOT_ARCH_OFFSET + 1],
        ]))
    } else {
        None
    };
    Some(Fadt { revision: header.revision, length: header.length, boot_arch })
}

/// IAPC_BOOT_ARCH bit 1: the machine has an 8042 (or something answering as one).
pub const BOOT_ARCH_8042: u16 = 1 << 1;

/// IAPC_BOOT_ARCH, bit by bit.
pub fn describe_boot_arch(v: u16, out: &mut Line) {
    let bit = |n: u16| u8::from(v & (1 << n) != 0);
    let _ = fmt::write(
        out,
        format_args!(
            "legacy={} 8042={} novga={} nomsi={} noaspm={} nocmos={}",
            bit(0),
            bit(1),
            bit(2),
            bit(3),
            bit(4),
            bit(5)
        ),
    );
}

// ---------------------------------------------------------------------------
// i8042
// ---------------------------------------------------------------------------

/// Status port bit 0: a byte is waiting at 0x60.
pub const ST_OUTPUT_FULL: u8 = 1 << 0;
/// Status port bit 1: the controller has not taken the last byte written yet.
pub const ST_INPUT_FULL: u8 = 1 << 1;
/// Status port bit 5: the waiting byte came from the auxiliary device (mouse, touchpad).
pub const ST_AUX: u8 = 1 << 5;

/// The status byte, bit by bit. `0xFF` is what an absent controller reads as.
///
/// Short names, to fit an 80-column line: `to` is the timeout bit, `par` the parity
/// error bit. Bits 3 (last write was a command) and 4 (keyboard not inhibited) are left
/// out; they say nothing about whether bytes arrive.
pub fn describe_i8042_status(s: u8, out: &mut Line) {
    if s == 0xFF {
        out.push_bytes(b"no controller answering");
        return;
    }
    let bit = |n: u8| u8::from(s & (1 << n) != 0);
    let _ = fmt::write(
        out,
        format_args!(
            "obf={} ibf={} aux={} sys={} to={} par={}",
            bit(0),
            bit(1),
            bit(5),
            bit(2),
            bit(6),
            bit(7)
        ),
    );
}

/// The controller configuration byte (command 0x20), bit by bit.
///
/// Bits 4 and 5 are *disable* bits: `kbd_off=1` means the keyboard clock is off and
/// nothing typed can arrive, however healthy the rest is. `kirq` / `airq` are the
/// keyboard and auxiliary interrupt enables, `xlat` is scancode translation to set 1.
pub fn describe_i8042_config(c: u8, out: &mut Line) {
    let bit = |n: u8| u8::from(c & (1 << n) != 0);
    let _ = fmt::write(
        out,
        format_args!(
            "kirq={} airq={} sys={} kbd_off={} aux_off={} xlat={}",
            bit(0),
            bit(1),
            bit(2),
            bit(4),
            bit(5),
            bit(6)
        ),
    );
}

/// The last few bytes seen on one channel, oldest first.
#[derive(Clone, Copy)]
pub struct ByteRing {
    buf: [u8; Self::CAP],
    /// Total pushed, ever. The ring holds the last `min(total, CAP)` of them.
    total: u32,
}

impl ByteRing {
    pub const CAP: usize = 16;

    #[must_use]
    pub const fn new() -> Self {
        Self { buf: [0; Self::CAP], total: 0 }
    }

    pub fn push(&mut self, b: u8) {
        self.buf[self.total as usize % Self::CAP] = b;
        self.total = self.total.wrapping_add(1);
    }

    #[must_use]
    pub fn total(&self) -> u32 {
        self.total
    }

    /// Write the held bytes as hex, oldest first, or `-` when there are none.
    pub fn write_hex(&self, out: &mut Line) {
        let held = (self.total as usize).min(Self::CAP);
        if held == 0 {
            out.push_bytes(b"-");
            return;
        }
        let start = self.total as usize - held;
        for i in 0..held {
            let b = self.buf[(start + i) % Self::CAP];
            let _ = fmt::write(out, format_args!("{b:02x} "));
        }
    }
}

impl Default for ByteRing {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// USB host controllers
// ---------------------------------------------------------------------------

/// The USB host controller generation, from the PCI programming interface.
#[must_use]
pub fn usb_kind(prog_if: u8) -> &'static str {
    match prog_if {
        0x00 => "UHCI",
        0x10 => "OHCI",
        0x20 => "EHCI",
        0x30 => "xHCI",
        0xFE => "USB-dev",
        _ => "USB-?",
    }
}

/// The UHCI legacy support register (PCI config 0xC0), the part that matters here.
///
/// Bit 4 enables the SMI that firmware keyboard emulation runs in, and bits 0..3 trap
/// accesses to ports 0x60 / 0x64. With them set, the firmware is the one answering on
/// those ports, not a real 8042. `traps` prints those four bits in the order
/// 60-read, 60-write, 64-read, 64-write.
pub fn describe_uhci_legsup(v: u16, out: &mut Line) {
    let bit = |n: u16| u8::from(v & (1 << n) != 0);
    let _ = fmt::write(
        out,
        format_args!(
            "legsup={v:#06x} smi={} traps={}{}{}{} pirq={}",
            bit(4),
            bit(0),
            bit(1),
            bit(2),
            bit(3),
            bit(13)
        ),
    );
}

/// EHCI / xHCI USBLEGSUP (and the control dword after it): who owns the controller.
/// `bios=1` is the "HC BIOS owned" semaphore, `os=1` the "HC OS owned" one.
pub fn describe_usb_legsup(legsup: u32, ctlsts: u32, out: &mut Line) {
    let _ = fmt::write(
        out,
        format_args!(
            "bios={} os={} smi_en={:#06x}",
            u8::from(legsup & (1 << 16) != 0),
            u8::from(legsup & (1 << 24) != 0),
            ctlsts & 0xFFFF
        ),
    );
}

// ---------------------------------------------------------------------------
// Odds and ends
// ---------------------------------------------------------------------------

/// CMOS RTC values are BCD unless status register B says otherwise.
#[must_use]
pub fn bcd_to_bin(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 0x0F)
}

/// Mnemonic for a CPU exception vector.
#[must_use]
pub fn exception_name(vector: u64) -> &'static str {
    const NAMES: [&str; 32] = [
        "#DE divide", "#DB debug", "NMI", "#BP breakpoint", "#OF overflow", "#BR bound",
        "#UD invalid opcode", "#NM no FPU", "#DF double fault", "coproc overrun",
        "#TS bad TSS", "#NP not present", "#SS stack fault", "#GP general protection",
        "#PF page fault", "reserved", "#MF x87", "#AC alignment", "#MC machine check",
        "#XM SIMD", "#VE virtualization", "#CP control prot", "reserved", "reserved",
        "reserved", "reserved", "reserved", "reserved", "#HV", "#VC", "#SX", "reserved",
    ];
    usize::try_from(vector).ok().and_then(|v| NAMES.get(v)).copied().unwrap_or("unknown")
}

/// Vectors for which the CPU pushes an error code. The stubs must agree with this.
#[must_use]
pub fn exception_has_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10..=14 | 17 | 21 | 29 | 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write;

    fn text(line: &Line) -> &str {
        core::str::from_utf8(line.as_bytes()).unwrap()
    }

    #[test]
    fn a_line_truncates_instead_of_overflowing() {
        let mut l = Line::new();
        for _ in 0..Line::CAP + 20 {
            let _ = l.write_str("x");
        }
        assert_eq!(l.len(), Line::CAP);
    }

    #[test]
    fn unprintable_bytes_become_dots() {
        let mut l = Line::new();
        l.push_bytes(b"a\x00b\xffc\n");
        assert_eq!(text(&l), "a.b.c.");
    }

    #[test]
    fn scale_matches_the_two_machines_it_is_for() {
        assert_eq!(pick_scale(1366, 768), 2, "Acer Aspire 5738 panel");
        assert_eq!(pick_scale(1920, 1080), 3, "full HD monitor");
        assert_eq!(pick_scale(1280, 800), 2, "QEMU's usual Limine mode");
        assert_eq!(pick_scale(800, 600), 1);
        assert_eq!(pick_scale(320, 200), 1, "never zero");
        assert_eq!(pick_scale(7680, 4320), 4, "capped");
    }

    #[test]
    fn firmware_types_are_named() {
        assert_eq!(firmware_name(0), "BIOS (legacy)");
        assert_eq!(firmware_name(2), "UEFI 64-bit");
        assert_eq!(firmware_name(99), "unknown");
    }

    #[test]
    fn the_memory_summary_counts_only_usable_ram_as_usable() {
        let mut m = MemSummary::default();
        m.add(0x0, 0x9_F000, 0);
        m.add(0x9_F000, 0x1000, 1);
        m.add(0x10_0000, 0x7FF0_0000, 0);
        m.add(0xFEE0_0000, 0x1000, 1);
        m.add(0x8000_0000, 0x1000, 42);
        assert_eq!(m.entries, 5);
        assert_eq!(m.usable_ranges, 2);
        assert_eq!(m.usable_bytes, 0x9_F000 + 0x7FF0_0000);
        assert_eq!(m.top_usable, 0x8000_0000);
        assert_eq!(m.per_type[1], 2);
        assert_eq!(m.other, 1);
    }

    fn rsdp_v2(xsdt: u64) -> [u8; 36] {
        let mut b = [0u8; 36];
        b[..8].copy_from_slice(b"RSD PTR ");
        b[9..15].copy_from_slice(b"BOCHS ");
        b[15] = 2;
        b[16..20].copy_from_slice(&0x7FE1_234Fu32.to_le_bytes());
        b[20..24].copy_from_slice(&36u32.to_le_bytes());
        b[24..32].copy_from_slice(&xsdt.to_le_bytes());
        let sum = b[..20].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[8] = 0u8.wrapping_sub(sum);
        b
    }

    #[test]
    fn an_acpi2_rsdp_is_read_with_both_pointers() {
        let r = parse_rsdp(&rsdp_v2(0x7FE1_3000)).unwrap();
        assert_eq!(r.revision, 2);
        assert_eq!(r.rsdt, 0x7FE1_234F);
        assert_eq!(r.xsdt, 0x7FE1_3000);
        assert!(r.checksum_ok);
    }

    #[test]
    fn an_acpi1_rsdp_has_no_xsdt_even_if_bytes_follow() {
        let mut b = rsdp_v2(0xDEAD_BEEF);
        b[15] = 0;
        let r = parse_rsdp(&b).unwrap();
        assert_eq!(r.xsdt, 0);
        assert!(!r.checksum_ok, "the revision byte changed and the checksum did not");
    }

    #[test]
    fn a_wrong_signature_is_not_an_rsdp() {
        let mut b = rsdp_v2(0);
        b[0] = b'X';
        assert_eq!(parse_rsdp(&b), None);
        assert_eq!(parse_rsdp(&b"RSD PTR "[..]), None, "too short");
    }

    fn fadt(revision: u8, length: u32, boot_arch: u16) -> [u8; 256] {
        let mut b = [0u8; 256];
        b[..4].copy_from_slice(b"FACP");
        b[4..8].copy_from_slice(&length.to_le_bytes());
        b[8] = revision;
        b[109..111].copy_from_slice(&boot_arch.to_le_bytes());
        b
    }

    #[test]
    fn the_fadt_boot_arch_flags_are_read_at_offset_109() {
        let f = parse_fadt(&fadt(4, 244, 0x0003)).unwrap();
        assert_eq!(f.boot_arch, Some(3));
        assert!(f.boot_arch_defined());
        assert_ne!(f.boot_arch.unwrap() & BOOT_ARCH_8042, 0);
    }

    #[test]
    fn an_acpi1_fadt_carries_the_bytes_but_not_the_meaning() {
        let f = parse_fadt(&fadt(1, 116, 0xFFFF)).unwrap();
        assert_eq!(f.boot_arch, Some(0xFFFF));
        assert!(!f.boot_arch_defined());
    }

    #[test]
    fn a_short_fadt_has_no_boot_arch() {
        let f = parse_fadt(&fadt(3, 100, 0x0002)).unwrap();
        assert_eq!(f.boot_arch, None);
        assert!(!f.boot_arch_defined());
    }

    #[test]
    fn only_facp_is_a_fadt() {
        let mut b = fadt(4, 244, 2);
        b[..4].copy_from_slice(b"APIC");
        assert_eq!(parse_fadt(&b), None);
    }

    #[test]
    fn boot_arch_bits_are_spelled_out() {
        let mut l = Line::new();
        describe_boot_arch(0x0002, &mut l);
        assert_eq!(text(&l), "legacy=0 8042=1 novga=0 nomsi=0 noaspm=0 nocmos=0");
    }

    #[test]
    fn an_absent_controller_is_called_that() {
        let mut l = Line::new();
        describe_i8042_status(0xFF, &mut l);
        assert!(text(&l).contains("no controller"));
    }

    #[test]
    fn status_bits_are_spelled_out() {
        let mut l = Line::new();
        describe_i8042_status(0x1C, &mut l);
        assert_eq!(text(&l), "obf=0 ibf=0 aux=0 sys=1 to=0 par=0");
        l.clear();
        describe_i8042_status(0x21, &mut l);
        assert_eq!(text(&l), "obf=1 ibf=0 aux=1 sys=0 to=0 par=0", "a mouse byte waiting");
    }

    #[test]
    fn the_config_byte_names_its_disable_bits_as_such() {
        let mut l = Line::new();
        // A typical value after the firmware: kbd IRQ on, system flag, aux clock off,
        // translation on.
        describe_i8042_config(0x65, &mut l);
        assert_eq!(text(&l), "kirq=1 airq=0 sys=1 kbd_off=0 aux_off=1 xlat=1");
    }

    #[test]
    fn the_ring_keeps_the_last_bytes_oldest_first() {
        let mut r = ByteRing::new();
        let mut l = Line::new();
        r.write_hex(&mut l);
        assert_eq!(text(&l), "-");
        for b in 0..20u8 {
            r.push(b);
        }
        l.clear();
        r.write_hex(&mut l);
        assert_eq!(r.total(), 20);
        assert!(text(&l).starts_with("04 05 06"));
        assert!(text(&l).ends_with("12 13 "));
    }

    #[test]
    fn usb_generations_come_from_prog_if() {
        assert_eq!(usb_kind(0x00), "UHCI");
        assert_eq!(usb_kind(0x10), "OHCI");
        assert_eq!(usb_kind(0x20), "EHCI");
        assert_eq!(usb_kind(0x30), "xHCI");
        assert_eq!(usb_kind(0x42), "USB-?");
    }

    #[test]
    fn uhci_legsup_shows_the_smi_and_port_traps() {
        let mut l = Line::new();
        describe_uhci_legsup(0x201F, &mut l);
        assert_eq!(text(&l), "legsup=0x201f smi=1 traps=1111 pirq=1");
        l.clear();
        describe_uhci_legsup(0x0011, &mut l);
        assert_eq!(text(&l), "legsup=0x0011 smi=1 traps=1000 pirq=0", "60-read trap only");
    }

    #[test]
    fn usb_legsup_says_who_owns_the_controller() {
        let mut l = Line::new();
        describe_usb_legsup(0x0001_0001, 0x0000_0001, &mut l);
        assert_eq!(text(&l), "bios=1 os=0 smi_en=0x0001");
    }

    #[test]
    fn bcd_is_decoded() {
        assert_eq!(bcd_to_bin(0x59), 59);
        assert_eq!(bcd_to_bin(0x00), 0);
        assert_eq!(bcd_to_bin(0x23), 23);
    }

    #[test]
    fn exceptions_are_named_and_out_of_range_is_not_a_panic() {
        assert_eq!(exception_name(13), "#GP general protection");
        assert_eq!(exception_name(14), "#PF page fault");
        assert_eq!(exception_name(300), "unknown");
    }

    #[test]
    fn the_error_code_vectors_are_the_architectural_ones() {
        let with: [u8; 10] = [8, 10, 11, 12, 13, 14, 17, 21, 29, 30];
        for v in 0..32u8 {
            assert_eq!(exception_has_error_code(v), with.contains(&v), "vector {v}");
        }
    }
}
