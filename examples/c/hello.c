/*
 * hello.c -- C demo for the ZPL ring-3 ABI.
 *
 * This needs a GCC cross-toolchain targeting ring 3 on this ABI, which
 * is not part of this repository, so the file is here as a source-level
 * demonstration rather than as something the build produces. The expected
 * compile invocation once the toolchain ships is:
 *
 *     x86_64-zpl-gcc -ffreestanding -nostdlib -static \
 *         -T crates/libzpl-sys/zpl.lds \
 *         examples/c/hello.c crates/libzpl-sys/libzpl.a -o hello.elf
 *
 * Until then the equivalent `.zpla` source in
 * `examples/programs/hello_ain.zpla` runs through `zpl-asm` end to
 * end.
 */

#include "libzpl.h"

static const uint8_t kHello[] = "hi";

void _start(void) {
    zpl_log(kHello, sizeof(kHello) - 1);
    zpl_exit(0);
}
