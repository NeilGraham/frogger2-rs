# libdragon ROM link for the Frogger 2 N64 port. Driven by tools/n64/build.sh:
#   make -C target/n64 -f tools/n64/Makefile CSRC=<crates/frogger-n64/csrc> RUST_LIB=<lib.a>
# Produces frogger2.z64 in the current directory (target/n64).
BUILD_DIR = build
SOURCE_DIR = $(CSRC)
N64_MKDFS_ROOT = filesystem
include $(N64_INST)/include/n64.mk

N64_ROM_TITLE = "FROGGER 2"
N64_ROM_EXPANSIONPAK = required
N64_ROM_SAVETYPE = none

CFLAGS += -I$(CSRC)

all: frogger2.z64

$(BUILD_DIR)/frogger2.dfs: $(wildcard filesystem/*)
$(BUILD_DIR)/frogger2.elf: $(BUILD_DIR)/main.o $(BUILD_DIR)/abi.o $(RUST_LIB)

frogger2.z64: $(BUILD_DIR)/frogger2.dfs

clean:
	rm -rf $(BUILD_DIR) frogger2.z64

-include $(wildcard $(BUILD_DIR)/*.d)

.PHONY: all clean
