//! The hardware diagnostic image's entry: Limine's answers, one numbered step each, then
//! `zpl_kernel::diag` for the hardware. Built with `--features hw_diag`; the kernel is
//! never entered.
//!
//! Each step is drawn before it runs (see `zpl_kernel::diag`), so the order below is the
//! order a photograph is read in. The framebuffer is the fifth step, not the first: the
//! steps before it are what make drawing safe, and they appear on screen the moment it
//! attaches.

use limine::firmware::{FIRMWARE_TYPE_EFI32, FIRMWARE_TYPE_EFI64, FIRMWARE_TYPE_X86BIOS};
use limine::request::{BootloaderInfoRequest, FirmwareTypeRequest, RsdpRequest};
use zpl_kernel::diag::{self, Color};

use crate::baremetal::{BASE_REVISION, FRAMEBUFFER_REQUEST, HHDM_REQUEST, MEMMAP_REQUEST};

#[used]
static FIRMWARE_TYPE_REQUEST: FirmwareTypeRequest = FirmwareTypeRequest::new();
#[used]
static BOOTLOADER_INFO_REQUEST: BootloaderInfoRequest = BootloaderInfoRequest::new();
#[used]
static RSDP_REQUEST: RsdpRequest = RsdpRequest::new();

pub fn run() -> ! {
    diag::early_init();

    diag::stage("Limine version, base revision");
    let name = BOOTLOADER_INFO_REQUEST.response();
    let supported = BASE_REVISION.is_supported();
    let actual = BASE_REVISION.actual_revision();
    let (bl_name, bl_version) = name.map_or(("?", "?"), |r| (r.name(), r.version()));
    diag::result(
        if supported { Color::Green } else { Color::Yellow },
        format_args!(
            "{bl_name} {bl_version}, base rev {} {}",
            limine::BaseRevision::MAX_SUPPORTED,
            match (supported, actual) {
                (true, _) => "accepted",
                (false, Some(_)) => "NOT accepted",
                (false, None) => "unknown (old loader?)",
            }
        ),
    );
    if let (false, Some(a)) = (supported, actual) {
        diag::detail(Color::Yellow, format_args!("loader runs base revision {a}"));
    }

    diag::stage("HHDM offset (direct map)");
    let hhdm = HHDM_REQUEST.response().map(|r| r.offset);
    match hhdm {
        Some(off) => {
            diag::set_hhdm(off);
            diag::result(Color::Green, format_args!("{off:#x}"));
        }
        None => diag::result(Color::Red, format_args!("no answer; addresses cannot be checked")),
    }

    diag::stage("framebuffer");
    attach_framebuffer();

    diag::stage("boot mode (firmware type)");
    match FIRMWARE_TYPE_REQUEST.response() {
        Some(r) => {
            let t = r.firmware_type;
            let color = match t {
                FIRMWARE_TYPE_X86BIOS | FIRMWARE_TYPE_EFI64 | FIRMWARE_TYPE_EFI32 => Color::Green,
                _ => Color::Yellow,
            };
            diag::result(color, format_args!("{} (type {t})", diag::decode::firmware_name(t)));
        }
        None => diag::result(Color::Yellow, format_args!("no answer from the loader")),
    }

    diag::stage("memory map");
    match MEMMAP_REQUEST.response() {
        Some(r) => {
            let mut it = r.entries().iter().map(|e| (e.base, e.length, e.type_));
            diag::report_memmap(&mut it);
        }
        None => diag::result(Color::Red, format_args!("no answer from the loader")),
    }

    diag::stage("ACPI RSDP pointer");
    let rsdp = RSDP_REQUEST.response().map(|r| r.address as u64);
    match rsdp {
        Some(a) => diag::result(Color::Green, format_args!("{a:#x}")),
        None => diag::result(Color::Yellow, format_args!("none given")),
    }

    diag::run_probes(rsdp)
}

fn attach_framebuffer() {
    let Some(resp) = FRAMEBUFFER_REQUEST.response() else {
        diag::result(Color::Red, format_args!("no answer: COM1 only"));
        return;
    };
    let Some(fb) = resp.framebuffers().first() else {
        diag::result(Color::Red, format_args!("none offered: COM1 only"));
        return;
    };
    let va = fb.address() as u64;
    let bytes = fb.pitch.saturating_mul(fb.height);
    // Checked rather than trusted: drawing into an unmapped framebuffer is a page fault
    // before anything is on screen to say so.
    if !diag::mapped(va, bytes) {
        diag::result(Color::Red, format_args!("address {va:#x} not mapped: COM1 only"));
        return;
    }
    diag::serial(format_args!(
        "  fb {va:#x} {}x{} pitch {} bpp {} shifts r{} g{} b{}",
        fb.width,
        fb.height,
        fb.pitch,
        fb.bpp,
        fb.red_mask_shift,
        fb.green_mask_shift,
        fb.blue_mask_shift
    ));
    diag::attach_framebuffer(
        va,
        fb.width,
        fb.height,
        fb.pitch,
        fb.bpp,
        fb.red_mask_shift,
        fb.green_mask_shift,
        fb.blue_mask_shift,
    );
}
