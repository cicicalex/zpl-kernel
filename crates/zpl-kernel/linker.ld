ENTRY(_zpl_qemu_boot)

SECTIONS
{
  . = 1M;

  .note : ALIGN(4) {
    *(.note .note.*)
  }

  .text : ALIGN(4K) {
    *(.text .text.*)
  }

  .rodata : ALIGN(4K) {
    *(.rodata .rodata.*)
  }

  .data : ALIGN(4K) {
    *(.data .data.*)
  }

  .bss : ALIGN(4K) {
    *(.bss .bss.*)
    *(COMMON)
  }
}
