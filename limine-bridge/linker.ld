/* Limine requires ELF program headers with virtual addresses in the high half
 * (p_vaddr >= 0xffffffff80000000). Lower-half layouts (e.g. QEMU -kernel Multiboot)
 * must not be reused for the ISO / limine-bridge path.
 *
 * LMA is kept in low physical RAM (starting at 1 MiB) via AT(...); VMA is high.
 * lld expects `AT(...)` immediately after `:`, before `ALIGN(...)`.
 *
 * Explicit PHDRS: Limine rejects a single PT_LOAD mixing RX/R/RW across one page.
 * Three PT_LOAD segments — text (RX), rodata (R), data+bss (RW).
 */
KERNEL_VMA = 0xffffffff80000000;

ENTRY(_start)

PHDRS
{
  text   PT_LOAD FLAGS(5);   /* PF_R | PF_X */
  rodata PT_LOAD FLAGS(4);   /* PF_R */
  data   PT_LOAD FLAGS(6);   /* PF_R | PF_W */
}

SECTIONS
{
  . = KERNEL_VMA + 0x100000;

  .text : AT(ADDR(.text) - KERNEL_VMA) ALIGN(4K) {
    *(.text .text.*)
  } :text

  . = ALIGN(4K);

  .rodata : AT(ADDR(.rodata) - KERNEL_VMA) ALIGN(4K) {
    *(.rodata .rodata.*)
    *(.note .note.*)
  } :rodata

  . = ALIGN(4K);

  .data : AT(ADDR(.data) - KERNEL_VMA) ALIGN(4K) {
    *(.data .data.*)
  } :data

  .bss : AT(ADDR(.bss) - KERNEL_VMA) ALIGN(4K) {
    *(.bss .bss.*)
    *(COMMON)
  } :data
}
