//! Limine requests and handoff — only built for `target_os = "none"`.
//!
//! This binary is linked at **higher-half VMA** (`0xffffffff80000000+`, see `limine-bridge/linker.ld`)
//! so Limine accepts the ELF. Limine maps that VA before calling [`_start`]; [`HhdmRequest`]
//! still supplies the offset for [`paging::set_hhdm_offset`] before entering `zpl-kernel`.
//!
//! **Bring-up:** [`bridge_com1_emit_ascii`] writes ASCII to **COM1** (`0x3F8`) **without** LSR polling
//! (QEMU `-serial file` may never report THR-empty; bounded wait starved `[BRIDGE-*]` lines).
//!
//! **Debug:** SeaBIOS+QEMU ISO often enables **VBE linear FB** (`vbe: Framebuffer address …`) so
//! blindly writing **physical** `0xB8000` before Limine can **hang** the guest (serial showed `@` then
//! silence). VGA text line **`ZPL-BRIDGE-START`** is painted **after** [`HHDM_REQUEST`] using
//! **`hhdm_offset + 0xB8000`** (Limine HHDM rule). Ultra-early proof is raw **`@`** on COM1 then
//! **`[BRIDGE-*]`** lines using **THR writes without LSR polling** (QEMU `-serial file` may never set
//! THR-empty).

use core::hint::spin_loop;

use limine::BaseRevision;
use limine::memmap::{Entry, MEMMAP_BOOTLOADER_RECLAIMABLE, MEMMAP_USABLE};
use limine::request::{
    ExecutableAddressRequest, FramebufferRequest, HhdmRequest, MemmapRequest, MemmapRespData,
    Response,
};

use zpl_kernel::mm::paging;
use zpl_kernel::publish_external_boot_frame_v1;
use zpl_kernel::start::zpl_boot_entry_v1;
use zpl_kernel::ExternalBootFrameV1;

pub(crate) static BASE_REVISION: BaseRevision = BaseRevision::new();

pub(crate) static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

pub(crate) static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

static EXE_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

/// v0.4: ask Limine for the graphical framebuffer so the kernel can draw text the monitor
/// actually shows. Without this request the screen stays black on real hardware even
/// though the kernel is alive on COM1 (observed 20 Sep 2026).
pub(crate) static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

/// Paint one 16-character VGA text row at **HHDM + 0xB8000** (Limine-mapped text buffer).
///
/// **QEMU CD + SeaBIOS VBE:** `offset` from Limine log (`Top of HHDM: 0x40000000`) is **not**
/// the same as `HhdmRespData.offset` semantics for `phys + offset` in all cases — calling this
/// after `set_hhdm_offset` **hung** the guest; keep for future once VA is confirmed on metal.
#[allow(dead_code)]
unsafe fn bridge_vga_line16_hhdm(hhdm_offset: u64, msg: &[u8; 16]) {
    let base = hhdm_offset.wrapping_add(0xB8000) as usize as *mut u16;
    let attr: u16 = 0x0F00;
    for (i, &b) in msg.iter().enumerate() {
        unsafe {
            core::ptr::write_volatile(base.add(i), attr | (b as u16));
        }
    }
}

/// COM1 THR write without LSR polling -- Limine + QEMU `-serial file` may never set
/// THR-empty.
unsafe fn bridge_com1_raw_putc_nowait(byte: u8) {
    // SAFETY: COM1, and nothing else in this image is writing to it at this point.
    unsafe { zpl_kernel::arch::port::outb(0x3F8, byte) }
}

unsafe fn bridge_com1_emit_ascii(s: &[u8]) {
    for &b in s {
        unsafe {
            bridge_com1_raw_putc_nowait(b);
        }
    }
}

/// Emit `prefix` + 16 lowercase hex nibbles (MSB first) + `suffix` — no alloc (HHDM evidence).
unsafe fn bridge_com1_emit_u64_hex_wrapped(prefix: &[u8], v: u64, suffix: &[u8]) {
    const DIGIT: &[u8; 16] = b"0123456789abcdef";
    unsafe {
        bridge_com1_emit_ascii(prefix);
        for n in (0..16).rev() {
            let idx = ((v >> (n * 4)) & 0xf) as usize;
            bridge_com1_raw_putc_nowait(DIGIT[idx]);
        }
        bridge_com1_emit_ascii(suffix);
    }
}

/// `#[repr(C)]` mirror of [`limine::request::MemmapRespData`] (crate-private fields).
#[repr(C)]
struct MemmapRespWire {
    entry_count: u64,
    entries: *const (),
}

/// Limine exposes `entries` as a pointer to a **dense** array of [`Entry`]
/// (`limine.c`: `memmap_response->entries = reported_addr(memmap_list)`).
///
/// [`MemmapRespData::entries()`] in limine-rs builds `&[&Entry]` from that pointer, which
/// does not match bootloader memory; the first [`build_external_frame`] walk faulted before
/// `[BRIDGE-PRE-KERNEL]`.
unsafe fn memmap_entries_flat(resp: &Response<MemmapRespData>) -> &'static [Entry] {
    const REV_SIZE: usize = core::mem::size_of::<u64>();
    let base = (resp as *const Response<MemmapRespData>).cast::<u8>();
    let wire = unsafe { &*base.add(REV_SIZE).cast::<MemmapRespWire>() };
    let n = wire.entry_count as usize;
    if n == 0 || wire.entries.is_null() {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(wire.entries.cast::<Entry>(), n) }
}

fn usable_chunk_bytes(e: &Entry) -> u64 {
    if e.type_ == MEMMAP_USABLE || e.type_ == MEMMAP_BOOTLOADER_RECLAIMABLE {
        e.length
    } else {
        0
    }
}

/// Hand the kernel's frame allocator the ranges the firmware called usable.
///
/// Until this existed, the whole memory map was boiled down to two numbers -- how much
/// low memory, how much high -- and the kernel assumed one gigabyte from its base was
/// its own. Under QEMU that is nearly true. On a real machine it is not: the firmware
/// keeps regions for itself, and allocating out of one corrupts whatever lives there,
/// with no fault and no message.
///
/// Only MEMMAP_USABLE is passed on. MEMMAP_BOOTLOADER_RECLAIMABLE is deliberately left
/// out even though it becomes free eventually: it still holds Limine's own structures
/// while this code runs, including the memory map being read right here. Treating it as
/// free would let the kernel allocate the very list it is being told about. That memory
/// is recoverable later, by a kernel that has finished with everything Limine left it.
/// This one has not.
fn declare_usable_ranges(entries: &[*const Entry]) -> usize {
    let mut ranges = [(0u64, 0u64); zpl_kernel::mm::frame_alloc::MAX_USABLE_RANGES];
    let mut n = 0;
    // Limine hands back an array of POINTERS to entries, not a dense array of them.
    // The code here read it as dense, which made every field an address: the first
    // "base" came out as 0xffff_8000_0ff7_a168, a higher-half pointer, not memory.
    // Printing the number is what showed it -- the totals this fed were wrong from the
    // day they were written, and nothing checked them.
    for slot in entries.iter() {
        let e: &Entry = unsafe { &**slot };
        if e.type_ != MEMMAP_USABLE || e.length == 0 {
            continue;
        }
        if n == ranges.len() {
            break;
        }
        ranges[n] = (e.base, e.base.saturating_add(e.length));
        unsafe {
            bridge_com1_emit_u64_hex_wrapped(b"[MMAP-USABLE-BASE=", e.base, b"]");
            bridge_com1_emit_u64_hex_wrapped(b"[LEN=", e.length, b"]");
            bridge_com1_emit_ascii(b"
");
        }
        n += 1;
    }
    zpl_kernel::mm::frame_alloc::declare_usable(&ranges[..n]);
    n
}

fn build_external_frame(entries: &[Entry]) -> ExternalBootFrameV1 {
    let mut usable_regions: u32 = 0;
    let mut total_bytes: u64 = 0;
    for e in entries.iter() {
        let n = usable_chunk_bytes(e);
        if n > 0 {
            usable_regions = usable_regions.saturating_add(1);
            total_bytes = total_bytes.saturating_add(n);
        }
    }
    if usable_regions == 0 || total_bytes == 0 {
        return ExternalBootFrameV1::minimal();
    }

    let total_kib_u64 = (total_bytes / 1024).max(640);
    let lower = 640u32.min(total_kib_u64.min(u32::MAX as u64) as u32);
    let upper_u64 = total_kib_u64.saturating_sub(lower as u64);
    let upper = upper_u64.min(u32::MAX as u64) as u32;

    ExternalBootFrameV1 {
        magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
        lower_memory_kib: lower,
        upper_memory_kib: upper,
        cpu_count: 1,
        memory_map_entries: usable_regions.max(1),
    }
}

#[no_mangle]
// With `hw_diag` the diagnostic takes over right after SSE and never returns, so the
// rest of this function is unreachable in that build -- deliberately, and only there.
#[cfg_attr(feature = "hw_diag", allow(unreachable_code))]
pub extern "C" fn _start() -> ! {
    // SSE enable: required before ANY Rust code in release mode.
    //   - Rust auto-vectorizes copies with MOVAPS/MOVUPS (e.g. frame_alloc::self_check)
    //   - extern "x86-interrupt" prologue saves xmm0..15 via MOVAPS (interrupts::early_ud et al)
    // Without CR0.MP=1, CR0.EM=0, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1, MOVAPS raises #UD.
    // Multiboot path enables this in zpl-kernel/start.rs (or rax, 0x600).
    // Symptoms when missing: 168x #UD recursion at handler prologue → stack overflow → triple fault.
    unsafe {
        core::arch::asm!(
            "mov rax, cr0",
            "and rax, {em_clr}",
            "or rax, {mp_set}",
            "mov cr0, rax",
            "mov rax, cr4",
            "or rax, {osfx}",
            "mov cr4, rax",
            em_clr = const !(1u64 << 2),
            mp_set = const 1u64 << 1,
            osfx = const (1u64 << 9) | (1u64 << 10),
            out("rax") _,
            options(nomem, nostack, preserves_flags),
        );
    }
    // The diagnostic image stops here and never reaches the kernel. Right after SSE,
    // because the diagnostic is ordinary Rust; before everything else, because
    // everything else is what it is there to watch.
    #[cfg(feature = "hw_diag")]
    crate::diag::run();
    // Earliest COM1 bytes (no LSR wait): '$' = SSE configured, '@' = legacy path proof.
    //
    // These two stay as raw assembly on purpose, and are the only port writes in the
    // tree outside `zpl_kernel::arch::port`. They run before anything else in the image
    // has been set up, and their whole job is to prove that this much of the boot
    // happened. Routing them through a call in another crate would make them depend on
    // that call being inlined -- which it would be, but "would be" is not the standard
    // for the two bytes whose only purpose is to be believed.
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3f8",
            "mov al, 0x24",
            "out dx, al",
            options(nomem, nostack),
        );
    }
    // #region agent log
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3f8",
            "mov al, 0x40", // '@'
            "out dx, al",
            options(nomem, nostack),
        );
    }
    for _ in 0..50_000 {
        spin_loop();
    }
    // #endregion agent log
    // Bounded COM1 trace (still before Limine requests).
    unsafe {
        bridge_com1_emit_ascii(b"[BRIDGE-START]\n");
    }

    if !BASE_REVISION.is_supported() {
        if BASE_REVISION.actual_revision().is_none() {
            halt();
        }
    }

    unsafe {
        bridge_com1_emit_ascii(b"[BRIDGE-PRE-HHDM]\n");
    }
    let hhdm_resp = HHDM_REQUEST
        .response()
        .expect("[limine-bridge] missing HHDM response");
    unsafe {
        // Runtime: compare with Limine serial line "Top of HHDM: …" (not always identical to this field).
        bridge_com1_emit_u64_hex_wrapped(b"[HHDM-OFF=", hhdm_resp.offset, b"]\n");
    }
    paging::set_hhdm_offset(hhdm_resp.offset);
    unsafe {
        bridge_com1_emit_ascii(b"[BRIDGE-POST-HHDM]\n");
    }

    let mem_resp = MEMMAP_REQUEST
        .response()
        .expect("[limine-bridge] missing memory map response");
    unsafe {
        bridge_com1_emit_ascii(b"[BRIDGE-POST-MMAP]\n");
    }
    let slice: &[Entry] = unsafe { memmap_entries_flat(mem_resp) };
    unsafe {
        bridge_com1_emit_u64_hex_wrapped(b"[MMAP-N=", slice.len() as u64, b"]\n");
    }

    // The same pointer array, read as what it is.
    let ptrs: &[*const Entry] = unsafe {
        core::slice::from_raw_parts(slice.as_ptr().cast::<*const Entry>(), slice.len())
    };
    let usable_n = declare_usable_ranges(ptrs);
    unsafe {
        bridge_com1_emit_u64_hex_wrapped(b"[MMAP-USABLE-N=", usable_n as u64, b"]" );
        bridge_com1_emit_ascii(b"
");
    }

    let exe_resp = EXE_ADDRESS_REQUEST
        .response()
        .expect("[limine-bridge] missing executable address response");
    unsafe {
        bridge_com1_emit_u64_hex_wrapped(b"[KERN-VBASE=", exe_resp.virtual_base, b"]\n");
        bridge_com1_emit_u64_hex_wrapped(b"[KERN-PBASE=", exe_resp.physical_base, b"]\n");
    }
    paging::set_kernel_image_offset(exe_resp.virtual_base, exe_resp.physical_base);

    // v0.4: hand the framebuffer to `zpl_kernel::drivers::fbcon`. Every value is also printed on
    // COM1 (hex) so a black screen can be told apart from a wrong mode: if `bpp` is not
    // 0x20 the console refuses to draw and the log says why.
    match FRAMEBUFFER_REQUEST.response() {
        Some(fb_resp) => match fb_resp.framebuffers().first() {
            Some(fb) => {
                let addr = fb.address() as u64;
                unsafe {
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-addr=", addr, b"]\n");
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-width=", fb.width, b"]\n");
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-height=", fb.height, b"]\n");
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-pitch=", fb.pitch, b"]\n");
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-bpp=", u64::from(fb.bpp), b"]\n");
                }
                zpl_kernel::drivers::fbcon::publish_framebuffer(
                    addr,
                    fb.width,
                    fb.height,
                    fb.pitch,
                    fb.bpp,
                    fb.red_mask_shift,
                    fb.green_mask_shift,
                    fb.blue_mask_shift,
                );
                unsafe {
                    if zpl_kernel::drivers::fbcon::is_fb_ready() {
                        bridge_com1_emit_ascii(b"[ZPL-V04 fbcon ready]\n");
                    } else {
                        bridge_com1_emit_ascii(b"[ZPL-V04 fbcon unsupported-mode]\n");
                    }
                }
                // One write + read-back at pixel 0, to prove the mapping is live before
                // the console draws anything. During bring-up this is what showed the
                // mapping was fine and the blank screen was the console's own fault.
                unsafe {
                    let p = addr as *mut u32;
                    core::ptr::write_volatile(p, 0x00FF_FFFF);
                    let back = core::ptr::read_volatile(p) as u64;
                    core::ptr::write_volatile(p, 0x0000_0000);
                    bridge_com1_emit_u64_hex_wrapped(b"[ZPL-V04 fb-readback=", back, b"]\n");
                }
            }
            None => unsafe {
                bridge_com1_emit_ascii(b"[ZPL-V04 fbcon no-framebuffer]\n");
            },
        },
        None => unsafe {
            bridge_com1_emit_ascii(b"[ZPL-V04 fbcon no-response]\n");
        },
    }

    let frame = build_external_frame(slice);
    let ptr = publish_external_boot_frame_v1(&frame);
    unsafe {
        bridge_com1_emit_ascii(b"[BRIDGE-PRE-KERNEL]\n");
        zpl_boot_entry_v1(ptr);
    }
}

fn halt() -> ! {
    loop {
        spin_loop();
    }
}
