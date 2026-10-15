# libdragon ROM link for the Frogger 2 N64 port. Driven by tools/n64/build.sh:
#   make -C target/n64 -f tools/n64/Makefile CSRC=<crates/frogger-n64/csrc> RUST_LIB=<lib.a>
# Produces frogger2.z64 in the current directory (target/n64).
# This Makefile (f2.ld is regenerated when its hot-region list changes).
F2_MAKEFILE := $(lastword $(MAKEFILE_LIST))
BUILD_DIR = build
SOURCE_DIR = $(CSRC)
# DFS root (build.sh --runtime passes N64_MKDFS_ROOT=runtime-fs: the data-free runtime ROM).
N64_MKDFS_ROOT = filesystem
# magma (libdragon's RSP 3D pipeline, used by r3d.c) is a preview API.
LIBDRAGON_PREVIEW = 2
include $(N64_INST)/include/n64.mk

N64_ROM_TITLE = "FROGGER 2"
N64_ROM_EXPANSIONPAK = required
N64_ROM_SAVETYPE = sram256k

CFLAGS += -I$(CSRC) -I$(BUILD_DIR)
# Measurement switches from the environment (e.g. F2N64_CFLAGS=-DF2N64_NO_TRIS).
CFLAGS += $(F2N64_CFLAGS)
# No home directory or user name in the ROM (assert texts, backtraces): paths relative to the
# repo (crates/...) and libdragon's install (libdragon/...). GCC lets the last match win.
F2_ROOT := $(abspath $(CSRC)/../../..)
F2_PATHMAP := -ffile-prefix-map=$(HOME)=~ -ffile-prefix-map=$(abspath $(N64_INST))=libdragon -ffile-prefix-map=$(F2_ROOT)/=
CFLAGS += $(F2_PATHMAP)
ASFLAGS += $(F2_PATHMAP)

# RSP ucodes (rsp_*.S are assembled as RSP code by n64.mk).
RSP_OBJS = $(BUILD_DIR)/rsp_f2mdx.o $(BUILD_DIR)/rsp_f2clip.o $(BUILD_DIR)/rsp_f2modge.o

all: frogger2.z64

# IMEM offset of each ucode's magma overlay text (_MG_OVERLAY), DMEM addresses, for r3d.c.
$(BUILD_DIR)/r3d_rsp_syms.h: $(RSP_OBJS) $(F2_MAKEFILE)
	@echo "    [SYMS] $@"
	@{ for u in rsp_f2mdx rsp_f2clip rsp_f2modge; do \
		a=$$($(N64_NM) $(BUILD_DIR)/$$u.elf | awk '$$3 == "_MG_OVERLAY" { print $$1 }'); \
		s=$$($(N64_NM) $(BUILD_DIR)/$$u.elf | awk '$$3 == "_start" { print $$1 }'); \
		echo "#define $$(echo $$u | tr a-z A-Z)_MG_OVERLAY_OFFSET (0x$$a - 0x$$s)"; \
	done; \
	a=$$($(N64_NM) $(BUILD_DIR)/rsp_f2modge.elf | awk '$$3 == "_MG_OVERLAY" { print $$1 }'); \
	e=$$($(N64_NM) $(BUILD_DIR)/rsp_f2modge.elf | awk '$$3 == "F2_ModgeEntry" { print $$1 }'); \
	echo "#define RSP_F2MODGE_ENTRY_OFFSET (0x$$e - 0x$$a)"; \
	m=$$($(N64_NM) $(BUILD_DIR)/rsp_f2mdx.elf | awk '$$3 == "F2_MTX_I" { print $$1 }'); \
	echo "#define RSP_F2MDX_F2_MTX_I_DMEM (0x$$m & 0xFFF)"; \
	c=$$($(N64_NM) $(BUILD_DIR)/rsp_f2mdx.elf | awk '$$3 == "MG_CLIPPING_TEXT" { print $$1 }'); \
	echo "#define RSP_F2MDX_MG_CLIPPING_TEXT_DMEM (0x$$c & 0xFFF)"; } > $@

$(BUILD_DIR)/r3d.o: $(BUILD_DIR)/r3d_rsp_syms.h

# Linker script: libdragon's n64.ld with the per-frame hot code first in .text (phase 3P).
# The VR4300's 16 KiB I-cache is direct-mapped: wherever two functions of the same loop
# share cache lines they evict each other on every call, and with the default (link order)
# placement that changed with every code edit (measured: the r3d flush 2.6 vs 9.6 ms per
# frame between two otherwise identical builds). Two contiguous regions, each < 16 KiB:
# `.text.f2hot_d` + `.text.f2hot_q` (the draw loop's actor path and r3d queueing) and `.text.f2hot_f` (flushing the
# lists to the RSP and drawing the CPU batches between them), then the libdragon / newlib
# functions of those loops. Order of the first region (phase 3E): r3d queueing first, then
# cull_world's closure (its own section, per draw), then the actor path. Since 3R the r3d
# batch (`run_queue_with`, `f2hot_q`) runs after the draw loop, so the two no longer share a
# loop: what matters (3D2) is that everything the draw loop runs per actor (`f2hot_d`:
# draw_listed_actor, Renderer::draw_actor, xform_actor_cached, quat_slerp, sin/acos_fast, the
# r3d recording, `queue_raw`) is contiguous and under 16 KiB (~13 KiB from `queue_raw`).
# Then `.text.f2hot_s` (< 16 KiB): the simulation's per-actor pass (`Game::copy_and_draw_actors`
# and what it calls per actor), contiguous so its functions never evict each other.
# Then the enemy loop (`.text.f2hot_e`), the fixed-point maths both it and the platform loop
# call per entity (`.text.f2hot_x`: MakeUnit, Orientate, IQuatSlerp, ...) and the platform
# loop (`.text.f2hot_p`): e + x and x + p each < 16 KiB (phase 3S3), so neither loop's code
# evicts itself.
HOT_C = n64_r3d_draw_list_impl load_frame upload_tex additive_tex draw_inline rspq_block_run mg_uniform_load mg_uniform_load_raw \
	mg_uniform_load_inline mg_uniform_load_inline_raw mg_draw_begin mg_draw_end \
	mg_bind_vertex_buffer data_cache_hit_writeback rspq_flush_internal __rspq_deferred_poll \
	n64_draw_tris_impl n64_draw_tl_tris_impl apply_state rdpq_triangle rdpq_triangle_rsp __rdpq_write8 \
	__rdpq_autosync_change memcpy memset
$(BUILD_DIR)/f2.ld: $(N64_LIBDIR)/n64.ld $(F2_MAKEFILE)
	@mkdir -p $(BUILD_DIR)
	@awk -v hot="$(HOT_C)" 'BEGIN { n = split(hot, h, " ") } \
		!done && $$0 ~ /^[ \t]*\*\(\.text\)[ \t]*$$/ { \
			print "        __f2_hot_start = .;"; \
			print "        *(.text.f2hot_q .text.f2hot_q.*)"; \
			print "        *(.text.*R3d*cull_world*)"; \
			print "        *(.text.f2hot_d .text.f2hot_d.*)"; \
			print "        __f2_hot_flush = .;"; \
			print "        *(.text.f2hot_f .text.f2hot_f.*)"; \
			for (i = 1; i <= n; i++) print "        *(.text." h[i] ")"; \
			print "        __f2_hot_end = .;"; \
			print "        *(.text.f2hot_s .text.f2hot_s.*)"; \
			print "        __f2_hot_sim_e = .;"; \
			print "        *(.text.f2hot_e .text.f2hot_e.*)"; \
			print "        __f2_hot_sim_x = .;"; \
			print "        *(.text.f2hot_x .text.f2hot_x.*)"; \
			print "        __f2_hot_sim_p = .;"; \
			print "        *(.text.f2hot_p .text.f2hot_p.*)"; \
			print "        __f2_hot_sim_end = .;"; done = 1 } { print }' $< > $@
N64_LDFLAGS := $(subst -Tn64.ld,-T$(CURDIR)/$(BUILD_DIR)/f2.ld,$(N64_LDFLAGS))
# Large-block pool (csrc/main.c): every malloc in the program, libdragon included.
N64_LDFLAGS += --wrap malloc --wrap free --wrap memalign --wrap calloc --wrap realloc

$(BUILD_DIR)/frogger2.dfs: $(wildcard $(N64_MKDFS_ROOT)/*)
$(BUILD_DIR)/frogger2.elf: $(BUILD_DIR)/main.o $(BUILD_DIR)/r3d.o $(BUILD_DIR)/abi.o $(RSP_OBJS) $(RUST_LIB) $(BUILD_DIR)/f2.ld

frogger2.z64: $(BUILD_DIR)/frogger2.dfs

clean:
	rm -rf $(BUILD_DIR) frogger2.z64

-include $(wildcard $(BUILD_DIR)/*.d)

.PHONY: all clean
