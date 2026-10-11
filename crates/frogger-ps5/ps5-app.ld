/*
 * Intermediate PIE layout for the PS5 native app, converted to the console's executable
 * format by ps5-native-tool (tools/ps5/build.sh). Based on ps5-native-app-boilerplate's
 * tooling/native/ps5-pie.ld (GPL-3.0-or-later, BlackBearReloaded / PS5_Vulkan), changed for
 * Rust std, which uses native (static) TLS rather than emulated TLS:
 *  - a PT_TLS header, so lld can resolve TLS relocations at all;
 *  - .tdata and .tbss in one 32-byte aligned output section padded to 32 bytes: the
 *    converter gives PT_TLS an alignment of at least 0x20 and the loader rounds the
 *    block to it, so lld's local-exec offsets only agree when the block's size and
 *    alignment are already multiples of 32 (as in crates/frogger-ps4/link.x);
 *  - room after the RELRO sections for the process parameters the converter adds.
 * The converter itself is patched to load the TLS image with the data segment
 * (tools/ps5/ps5-native-tool-tls.patch).
 */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)

PHDRS {
    text PT_LOAD FLAGS (0x5);
    ro PT_LOAD FLAGS (0x4);
    relro PT_LOAD FLAGS (0x6);
    data PT_LOAD FLAGS (0x6);
    dynamic PT_DYNAMIC FLAGS (0x6);
    tls PT_TLS FLAGS (0x4);
}

SECTIONS {
    .text 0 : ALIGN(CONSTANT(MAXPAGESIZE)) {
        QUAD(0xcccccccccccccccc)
        QUAD(0xcccccccccccccccc)
        *(.text .text.*)
        *(.plt .plt.*)
    } : text

    .eh_frame_hdr : ALIGN(CONSTANT(MAXPAGESIZE)) {
        KEEP(*(.eh_frame_hdr))
    } : ro
    .eh_frame : { KEEP(*(.eh_frame)) } : ro
    .gcc_except_table : { *(.gcc_except_table .gcc_except_table.*) } : ro
    .rodata : { *(.rodata .rodata.*) } : ro

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .data.rel.ro : {
        *(.data.rel.ro .data.rel.ro.*)
    } : relro
    .got : { *(.got) } : relro
    .got.plt : { *(.got.plt) } : relro
    .preinit_array : {
        PROVIDE_HIDDEN(__preinit_array_start = .);
        KEEP(*(.preinit_array))
        PROVIDE_HIDDEN(__preinit_array_end = .);
    } : relro
    .init_array : {
        PROVIDE_HIDDEN(__init_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*) SORT_BY_INIT_PRIORITY(.ctors.*)))
        KEEP(*(.init_array .ctors))
        PROVIDE_HIDDEN(__init_array_end = .);
    } : relro
    .fini_array : {
        PROVIDE_HIDDEN(__fini_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*) SORT_BY_INIT_PRIORITY(.dtors.*)))
        KEEP(*(.fini_array .dtors))
        PROVIDE_HIDDEN(__fini_array_end = .);
    } : relro

    /* Kept only long enough for the converter to read and rewrite them. */
    .dynsym : { *(.dynsym) } : relro
    .gnu.hash : { *(.gnu.hash) } : relro
    .dynstr : { *(.dynstr) } : relro
    .rela.dyn : { *(.rela.dyn) } : relro
    .rela.plt : { *(.rela.plt) } : relro
    /* The converter places its process parameters here. */
    . += 0x1000;

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .dynamic : {
        *(.dynamic)
    } : data : dynamic
    .data : { *(.data .data.*) } : data
    .tdata : ALIGN(32) {
        *(.tdata .tdata.*)
        *(.tbss .tbss.*)
        . = ALIGN(32);
    } : data : tls
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } : data
}

/* For the payload SDK's libunwind (GPU build: the driver's C++), which finds unwind
 * tables through these instead of dl_iterate_phdr (PS5_Vulkan ps5-pie-unwind.ld). */
PROVIDE(__eh_frame_start = ADDR(.eh_frame));
PROVIDE(__eh_frame_end = ADDR(.eh_frame) + SIZEOF(.eh_frame));
PROVIDE(__eh_frame_hdr_start = ADDR(.eh_frame_hdr));
PROVIDE(__eh_frame_hdr_end = ADDR(.eh_frame_hdr) + SIZEOF(.eh_frame_hdr));
