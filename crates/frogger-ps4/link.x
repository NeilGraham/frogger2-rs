/* OpenOrbis PS4 toolchain link.x (GPL-3.0, https://github.com/OpenOrbis/OpenOrbis-PS4-Toolchain)
 * adapted for Rust objects: Rust emits one section per symbol (.data.<sym>, .data.rel.ro.<sym>,
 * .gcc_except_table.<sym>); the original patterns only match the bare names, which leaves
 * those as orphans that break the segment layout (an unaligned RELRO/TLS segment that the
 * PS4 loader rejects). */
SECTIONS
{
	# Combine text, rodata, and eh_frame stuff
	# In case you'll ever need to get the base address of eboot.bin, you can use the &__text_start symbol.
	# No need to go through libkernel for that.
	.text : ALIGN(0x4000) {
		__text_start = .;
		# "/libexec/ld-elf.so.1"
		QUAD(0x6365786562696C2F);
		QUAD(0x2E666C652D646C2F);
		QUAD(0x00000000312E6F73);
		QUAD(0x0000000000000000);

		# original .text
		*(.text .text.*)
	}

	.rodata : ALIGN(0x10) {
		*(.rodata .rodata.*)
		*(.gcc_except_table .gcc_except_table.*)
	}

    # Since we lack a proper POSIX dladdr() on a PS4, we define these global symbols
    # For libunwind's bare metal compilation mode.
    # That's way better than going through libkernel... ;-;
	.eh_frame : {
	   __eh_frame_start = .;
	   KEEP(*(.eh_frame))
	   __eh_frame_end = .;
	}

	.eh_frame_hdr : {
	   __eh_frame_hdr_start = .;
	   KEEP(*(.eh_frame_hdr))
	   __eh_frame_hdr_end = .;
	}

	.data.rel.ro : ALIGN(0x4000) {
	   __data_relro_start = .;	
	   *(.data.rel.ro .data.rel.ro.*)
	   __data_relro_start = .;
	}
	
	.init_array : {
		*(.init_array);
	}
	
	.dynamic : {
		*(.dynamic);
	}

	/* No TLS: the Rust code is built with emulated TLS (tools/ps4/console-target.py,
	   crates/console-host/csrc/emutls.c), so the executable carries no PT_TLS and does not
	   depend on how the loader lays out the static TLS block (tools/ps4/build-pkg.sh
	   checks). A .tdata/.tbss input here would be an error. */
	.tdata : { *(.tdata .tdata.*) }
	.tbss : { *(.tbss .tbss.*) }
	ASSERT(SIZEOF(.tdata) + SIZEOF(.tbss) == 0, "native TLS in the PS4 build: everything must use emulated TLS")

	# Align .got to 0x4000 if .data.rel.ro doesn't exist
	.got : ALIGN(SIZEOF(.data.rel.ro) > 0 ? 8 : 0x4000) {
		*(.got)
	}

	# Align .got.plt to 0x4000 if .got doesn't exist
	.got.plt : ALIGN((SIZEOF(.got) > 0 || SIZEOF(.data.rel.ro) > 0) ? 8 : 0x4000) {
		*(.got.plt)
	}

	.data.sce_process_param : ALIGN(0x4000) {
		KEEP(*(.data.sce_process_param))
	}

	.data.sce_module_param : ALIGN(0x4000) {
		KEEP(*(.data.sce_module_param))
	}

	.data : {
		*(.data .data.*)
	}

	.bss : {
		*(.bss .bss.*)
	}

	# Force .got.plt to appear, because SPRX requires a valid .got.plt.
	/DISCARD/ : {
		QUAD(_GLOBAL_OFFSET_TABLE_)
	}
}
