# libdragon ROM link for the Frogger 2 N64 port. Driven by tools/n64/build.sh:
#   make -C target/n64 -f tools/n64/Makefile CSRC=<crates/frogger-n64/csrc> RUST_LIB=<lib.a>
# Produces frogger2.z64 in the current directory (target/n64).
BUILD_DIR = build
SOURCE_DIR = $(CSRC)
N64_MKDFS_ROOT = filesystem
# magma (libdragon's RSP 3D pipeline, used by r3d.c) is a preview API.
LIBDRAGON_PREVIEW = 2
include $(N64_INST)/include/n64.mk

N64_ROM_TITLE = "FROGGER 2"
N64_ROM_EXPANSIONPAK = required
N64_ROM_SAVETYPE = none

CFLAGS += -I$(CSRC) -I$(BUILD_DIR)

# RSP ucodes (rsp_*.S are assembled as RSP code by n64.mk).
RSP_OBJS = $(BUILD_DIR)/rsp_f2mdx.o $(BUILD_DIR)/rsp_f2clip.o

all: frogger2.z64

# IMEM offset of each ucode's magma overlay text (_MG_OVERLAY), for r3d.c.
$(BUILD_DIR)/r3d_rsp_syms.h: $(RSP_OBJS)
	@echo "    [SYMS] $@"
	@{ for u in rsp_f2mdx rsp_f2clip; do \
		a=$$($(N64_NM) $(BUILD_DIR)/$$u.elf | awk '$$3 == "_MG_OVERLAY" { print $$1 }'); \
		s=$$($(N64_NM) $(BUILD_DIR)/$$u.elf | awk '$$3 == "_start" { print $$1 }'); \
		echo "#define $$(echo $$u | tr a-z A-Z)_MG_OVERLAY_OFFSET (0x$$a - 0x$$s)"; \
	done; } > $@

$(BUILD_DIR)/r3d.o: $(BUILD_DIR)/r3d_rsp_syms.h

$(BUILD_DIR)/frogger2.dfs: $(wildcard filesystem/*)
$(BUILD_DIR)/frogger2.elf: $(BUILD_DIR)/main.o $(BUILD_DIR)/r3d.o $(BUILD_DIR)/abi.o $(RSP_OBJS) $(RUST_LIB)

frogger2.z64: $(BUILD_DIR)/frogger2.dfs

clean:
	rm -rf $(BUILD_DIR) frogger2.z64

-include $(wildcard $(BUILD_DIR)/*.d)

.PHONY: all clean
