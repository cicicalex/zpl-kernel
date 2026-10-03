//! Acceptance demo for the ELF loader (`elf_demo` feature). Loads a tiny
//! hand-crafted ELF64 image and transitions to its entry point in ring 3.
//!
//! The embedded image holds the same syscall sequence the ring-3 demo
//! uses (`zpl_log("hi")` + `zpl_exit(0)`) but it is delivered through the
//! ELF loader (`elf::load`) rather than copied byte-by-byte, exercising
//! the parser + segment mapping path.

// Also built for `user_programs`: the prompt needs the ELF images and the ring-3
// entry from this module, but NOT the boot branch that runs one automatically.
// Every `#[cfg(feature = "elf_demo")]` on the boot path stays as it was.
#![cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "elf_demo", feature = "user_programs")
))]

use core::arch::asm;

use crate::fs::elf;
use crate::mm::frame_alloc::alloc_frame;
use crate::mm::paging;

const USER_STACK_VIRT_BASE: u64 = paging::USER_REGION_BASE + 0x20_000;
const USER_STACK_TOP: u64 = USER_STACK_VIRT_BASE + 0x1000;

const USER_CODE_SELECTOR_DPL3: u64 = 0x18 | 3;
const USER_DATA_SELECTOR_DPL3: u64 = 0x20 | 3;
const USER_RFLAGS: u64 = 0x202;

/// Minimal valid ELF64 image: ELF header (64) + 1 program header (56) +
/// 31 bytes of code+data (29 instr + 2 "hi"). PT_LOAD maps virt
/// `0x40010000` directly. Entry = `0x40010000`.
///
/// Layout reference (file offsets):
///   0..16   e_ident
///   16..18  e_type=2 (ET_EXEC)
///   18..20  e_machine=0x3E
///   20..24  e_version=1
///   24..32  e_entry=0x40010000
///   32..40  e_phoff=64
///   40..48  e_shoff=0
///   48..52  e_flags=0
///   52..54  e_ehsize=64
///   54..56  e_phentsize=56
///   56..58  e_phnum=1
///   58..60  e_shentsize=0
///   60..62  e_shnum=0
///   62..64  e_shstrndx=0
///   64..120 program header
///   120..151 segment payload
/// The two images, both always compiled.
///
/// They used to be one name behind a feature, because only one was ever loaded.
/// The prompt can load either -- they are two files in the filesystem now -- so
/// both have to exist at once. `HELLO_ELF` stays as the alias the boot path uses,
/// so that path is unchanged.
pub const HELLO_ELF_BYTES: &[u8] = &[
    // e_ident
    0x7F, b'E', b'L', b'F', // magic
    2,    // EI_CLASS = ELF64
    1,    // EI_DATA = LSB
    1,    // EI_VERSION
    0,    // EI_OSABI = SysV
    0,    // EI_ABIVERSION
    0, 0, 0, 0, 0, 0, 0, // padding
    // e_type
    2, 0, // ET_EXEC
    // e_machine
    0x3E, 0, // EM_X86_64
    // e_version
    1, 0, 0, 0,
    // e_entry = 0x0000_0000_4001_0000
    0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
    // e_phoff = 64
    64, 0, 0, 0, 0, 0, 0, 0,
    // e_shoff = 0
    0, 0, 0, 0, 0, 0, 0, 0,
    // e_flags = 0
    0, 0, 0, 0,
    // e_ehsize = 64
    64, 0,
    // e_phentsize = 56
    56, 0,
    // e_phnum = 1
    1, 0,
    // e_shentsize = 0
    0, 0,
    // e_shnum = 0
    0, 0,
    // e_shstrndx = 0
    0, 0,
    //
    // ----- Program header (PT_LOAD) -----
    // p_type = 1 (PT_LOAD)
    1, 0, 0, 0,
    // p_flags = PF_R | PF_X | PF_W = 7 (writable since the loader will
    // also need to copy bytes; ring-3 keeps the user mapping writable).
    7, 0, 0, 0,
    // p_offset = 120
    120, 0, 0, 0, 0, 0, 0, 0,
    // p_vaddr = 0x4001_0000
    0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
    // p_paddr = 0x4001_0000
    0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
    // p_filesz = 31
    31, 0, 0, 0, 0, 0, 0, 0,
    // p_memsz = 31
    31, 0, 0, 0, 0, 0, 0, 0,
    // p_align = 1
    1, 0, 0, 0, 0, 0, 0, 0,
    //
    // ----- Segment payload (29 bytes of code + 2 bytes "hi") -----
    // mov eax, 0
    0xB8, 0x00, 0x00, 0x00, 0x00,
    // mov edi, 0x4001_001D (pointer to "hi" right after the code)
    0xBF, 0x1D, 0x00, 0x01, 0x40,
    // mov esi, 2
    0xBE, 0x02, 0x00, 0x00, 0x00,
    // int 0x80 (SYS_LOG)
    0xCD, 0x80,
    // mov eax, 4 (SYS_EXIT)
    0xB8, 0x04, 0x00, 0x00, 0x00,
    // mov edi, 0
    0xBF, 0x00, 0x00, 0x00, 0x00,
    // int 0x80 (SYS_EXIT)
    0xCD, 0x80,
    // "hi" bytes at virt 0x4001_001D / file offset 149
    b'h', b'i',
];

/// `attack.elf` — bit-identical to `cargo run -p zpl-asm -- examples/programs/attack.zpla`.
/// PT_LOAD maps virt `0x40010000`, p_filesz=53, segment payload =
/// 29-byte SYS_COMPUTE+SYS_EXIT program followed by a 24-byte
/// `ComputeInput` blob (bias≈0.95, dimension=9, samples=64, seed=0xA77AC4).
///
/// When the kernel is built with `--features attack_demo`, this ELF is
/// loaded by `elf::load` instead of the `hello.elf` payload above.
pub const ATTACK_ELF_BYTES: &[u8] = &[
    0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x3e, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x38, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00,
    0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x40,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
    0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x35, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xb8, 0x01, 0x00, 0x00, 0x00, 0xbf, 0x1d, 0x00, 0x01, 0x40, 0xbe, 0x00,
    0x00, 0x00, 0x00, 0xcd, 0x80, 0xb8, 0x04, 0x00, 0x00, 0x00, 0xbf, 0x00,
    0x00, 0x00, 0x00, 0xcd, 0x80, 0x33, 0x33, 0x33, 0x33, 0x33, 0x33, 0xee,
    0x3f, 0x09, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0xc4, 0x7a, 0x0a,
    0x00, 0x00, 0x00, 0x00, 0x00,
];

/// What the boot path loads, selected the way it always was.
#[cfg(not(feature = "attack_demo"))]
pub const HELLO_ELF: &[u8] = HELLO_ELF_BYTES;
#[cfg(feature = "attack_demo")]
pub const HELLO_ELF: &[u8] = ATTACK_ELF_BYTES;

#[derive(Debug, Clone, Copy)]
pub enum ElfDemoErr {
    NoStackFrame,
    MapStackFailed,
    LoadFailed(elf::ElfError),
}

/// Ensure user stack is mapped, then parse + load the embedded ELF.
/// On success, returns the entry virtual address.
///
/// # Safety
/// Caller must ensure paging::init has run and the TSS rsp0 is set up.
pub unsafe fn prepare() -> Result<u64, ElfDemoErr> {
    let stack_phys = alloc_frame().ok_or(ElfDemoErr::NoStackFrame)?;
    unsafe {
        paging::map_4k_user_page(USER_STACK_VIRT_BASE, stack_phys)
            .map_err(|_| ElfDemoErr::MapStackFailed)?;
    }
    let image = unsafe { elf::load(HELLO_ELF).map_err(ElfDemoErr::LoadFailed)? };
    Ok(image.entry)
}

/// Build an `iretq` frame for `entry` and jump.
///
/// # Safety
/// `prepare()` must have succeeded for the given entry.
#[inline(never)]
pub unsafe fn enter_ring3(entry: u64) -> ! {
    unsafe {
        asm!(
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = in(reg) USER_DATA_SELECTOR_DPL3,
            rsp = in(reg) USER_STACK_TOP,
            rflags = in(reg) USER_RFLAGS,
            cs = in(reg) USER_CODE_SELECTOR_DPL3,
            rip = in(reg) entry,
            options(noreturn),
        );
    }
}
