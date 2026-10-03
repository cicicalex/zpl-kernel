//! The first instructions: the entry points a bootloader jumps to.
//!
//! Three symbols, one per boot protocol, each taking a raw pointer from a
//! bootloader that this kernel has no way to validate -- which is why all three
//! are `unsafe extern "C"` and why their safety comments say what the caller has
//! to have arranged.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "qemu_boot"
))]
const BOOT_STACK_SIZE: usize = 16 * 1024;

/// Limine / higher-half consumers link `zpl-kernel` without `qemu_boot` so the
/// Multiboot `global_asm` (32-bit `offset` relocations) is omitted. Provide the
/// GDT image and PDPT storage the Rust pager and TSS paths expect.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot")
))]
#[no_mangle]
pub static mut zpl_gdt: [u64; 7] = [
    0,
    0x00AF9A000000FFFF,
    0x00AF92000000FFFF,
    0x00AFFA000000FFFF,
    0x00AFF2000000FFFF,
    0,
    0,
];

/// PDPT storage referenced by `paging::init` PML4[0] hook on Limine. Must be 4 KiB aligned
/// because PML4 entry encoding uses bits 51:12 for physical address; misalignment leaks bits
/// into RSVD/flags slots and triggers vec=14 err=0xa during walk.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot")
))]
#[repr(C, align(4096))]
pub struct ZplPdptPage(pub [u64; 512]);

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot")
))]
#[no_mangle]
pub static mut zpl_pdpt: ZplPdptPage = ZplPdptPage([0u64; 512]);

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "qemu_boot"
))]
core::arch::global_asm!(
    r#"
    .section .note.Xen,"a",@note
    .align 4
    .long 4, 4, 18
    .asciz "Xen"
    .align 4
    .long _zpl_qemu_boot
    .align 4

    .section .bss.stack,"aw",@nobits
    .balign 16
zpl_boot_stack:
    .space {stack_size}
zpl_boot_stack_end:

    .section .data.gdt,"aw",@progbits
    .balign 16
    .global zpl_gdt
    .global zpl_gdtr
zpl_gdt:
    .quad 0                         /* 0x00 null */
    .quad 0x00AF9A000000FFFF        /* 0x08 kernel code (DPL=0, 64-bit) */
    .quad 0x00AF92000000FFFF        /* 0x10 kernel data (DPL=0) */
    .quad 0x00AFFA000000FFFF        /* 0x18 user code (DPL=3, 64-bit) */
    .quad 0x00AFF2000000FFFF        /* 0x20 user data (DPL=3) */
    .quad 0                         /* 0x28 TSS low — patched by tss::init */
    .quad 0                         /* 0x30 TSS high */
zpl_gdt_end:

    .balign 16
zpl_gdtr:
    .word zpl_gdt_end - zpl_gdt - 1
    .quad zpl_gdt

    .balign 8
zpl_far_jmp_target:
    .long zpl_long_mode_entry
    .word 0x08

    .section .data.pagetables,"aw",@progbits
    .balign 4096
    .global zpl_pml4
    .global zpl_pdpt
    .global zpl_pd
zpl_pml4:
    .space 4096
zpl_pdpt:
    .space 4096
zpl_pd:
    .space 4096

    .section .text._start,"ax",@progbits
    .code32
    .global _zpl_qemu_boot
_zpl_qemu_boot:
    cli

    mov edi, offset zpl_pml4
    mov ecx, 4096 * 3 / 4
    xor eax, eax
    rep stosd

    /* PML4[0] = zpl_pdpt | P|RW|U so the user PDPT slot can be enabled
       at runtime (paging::init() sets U=1 only on PDPT[1] subtree, the
       2 MiB pages under PDPT[0] keep U=0 so user mode cannot see them). */
    mov eax, offset zpl_pdpt
    or  eax, 0x07
    mov [zpl_pml4], eax

    mov eax, offset zpl_pd
    or  eax, 0x03
    mov [zpl_pdpt], eax

    mov edi, offset zpl_pd
    xor ecx, ecx
1:
    mov eax, ecx
    shl eax, 21
    or  eax, 0x83
    mov [edi], eax
    mov dword ptr [edi+4], 0
    add edi, 8
    inc ecx
    cmp ecx, 512
    jne 1b

    mov eax, offset zpl_pml4
    mov cr3, eax

    mov eax, cr4
    or  eax, 0x20
    mov cr4, eax

    mov ecx, 0xC0000080
    rdmsr
    or  eax, 0x100
    wrmsr

    mov eax, cr0
    or  eax, 0x80000000
    mov cr0, eax

    lgdt [zpl_gdtr]
    mov eax, offset zpl_long_mode_entry
    push 0x08
    push eax
    retf

    .code64
zpl_long_mode_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    mov rax, cr0
    and rax, ~0x4
    or  rax, 0x2
    mov cr0, rax
    mov rax, cr4
    or  rax, 0x600
    mov cr4, rax

    lea rsp, [rip + zpl_boot_stack_end]
    and rsp, -16

    mov dx, 0x3F9
    mov al, 0x00
    out dx, al
    mov dx, 0x3FB
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 0x03
    out dx, al
    mov dx, 0x3F9
    mov al, 0x00
    out dx, al
    mov dx, 0x3FB
    mov al, 0x03
    out dx, al
    mov dx, 0x3FA
    mov al, 0xC7
    out dx, al
    mov dx, 0x3FC
    mov al, 0x03
    out dx, al

    call {entry}
2:
    hlt
    jmp 2b
"#,
    stack_size = const BOOT_STACK_SIZE,
    entry = sym zpl_kernel_rust_entry,
);

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "qemu_boot"
))]
#[no_mangle]
pub extern "C" fn zpl_kernel_rust_entry() -> ! {
    // QEMU `-kernel` path: identity-mapped bootstrap; keep kernel_va_to_pa(va) == va.
    crate::mm::paging::set_kernel_image_offset(0, 0);
    let boot_info = crate::arch::x86_64::collect_boot_info();
    crate::boot::kernel_entry(boot_info)
}

#[inline(always)]
fn fail_invalid_boot_handoff() -> ! {
    crate::arch::x86_64::halt_loop()
}

/// Optional protocol entrypoint for external bootloader integration.
/// A bootloader can call this symbol and pass a pointer to
/// `ExternalBootFrameV1` located in shared memory.
///
/// # Safety
/// `frame_ptr` must point to a valid, initialized `ExternalBootFrameV1`
/// structure that remains readable for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn zpl_boot_entry_v1(
    frame_ptr: *const crate::arch::x86_64::ExternalBootFrameV1,
) -> ! {
    // SAFETY: pointer is provided by bootloader.
    let boot_info = match unsafe { crate::arch::x86_64::collect_boot_info_from_ptr(frame_ptr) } {
        Some(info) => info,
        None => fail_invalid_boot_handoff(),
    };
    crate::boot::kernel_entry(boot_info)
}

/// Adapter entrypoint for Multiboot2-like handoff.
///
/// # Safety
/// `handoff_ptr` must point to a valid `Multiboot2HandoffV1` region produced
/// by the active boot stack.
#[no_mangle]
pub unsafe extern "C" fn zpl_boot_entry_multiboot2_v1(
    handoff_ptr: *const crate::mm::boot_stack::Multiboot2HandoffV1,
) -> ! {
    // SAFETY: pointer is provided by selected boot stack.
    let boot_info = match unsafe { crate::mm::boot_stack::boot_info_from_multiboot2_ptr(handoff_ptr) } {
        Some(info) => info,
        None => fail_invalid_boot_handoff(),
    };
    crate::boot::kernel_entry(boot_info)
}

/// Adapter entrypoint for Stivale2-like handoff.
///
/// # Safety
/// `handoff_ptr` must point to a valid `Stivale2HandoffV1` region produced by
/// the active boot stack.
#[no_mangle]
pub unsafe extern "C" fn zpl_boot_entry_stivale2_v1(
    handoff_ptr: *const crate::mm::boot_stack::Stivale2HandoffV1,
) -> ! {
    // SAFETY: pointer is provided by selected boot stack.
    let boot_info = match unsafe { crate::mm::boot_stack::boot_info_from_stivale2_ptr(handoff_ptr) } {
        Some(info) => info,
        None => fail_invalid_boot_handoff(),
    };
    crate::boot::kernel_entry(boot_info)
}
