#!/usr/bin/env bash
# Builds the PS5 Vulkan driver (Mesa RADV with the PS5 winsys, from the PS5_Vulkan
# project, GPL-3.0) as a static archive for linking into a PS5 title, plus the
# project's own RADV smoke-test title. Nothing is written into this repository:
# everything lives under $PS5VK_OPT (default ~/opt). See tools/ps5/VULKAN.md.
#
#   tools/ps5/build-vulkan-driver.sh            # deps + RADV release archive + smoke title
#   tools/ps5/build-vulkan-driver.sh debug      # the assertion-enabled archive instead
#
# Output:
#   $PS5VK/.deps/native/radv-release/lib/libvulkan_radeon.ps5.a   (≈270 MB, RADV+ACO+NIR+runtime)
#   $PS5VK/.deps/native/radv-release/include/vulkan/              (headers it was built with)
#   $PS5VK/.deps/native/ps5-payload-sdk/                          (the SDK fork the archive needs)
#   $PS5VK/build/host/ps5-native-tool, $PS5VK/runtime/libc.prx      (title link/sign tools)
#   $PS5VK/dist/PPSA99014/                                        (RADV smoke-test title)
#
# Version pins (as of PS5_Vulkan 2026-10-01, what this was verified with on 2026-10-10):
#   - PS5_Vulkan        github.com/mihawk-99/PS5_Vulkan (main)
#   - PS5_PayloadSDK    github.com/mihawk-99/PS5_PayloadSDK @ b83202be (upstream payload SDK
#                       v0.42 release zip + the author's platform layer libps5platform.a); NOT the
#                       stock v0.43 SDK in ~/opt/ps5-payload-sdk. A title linking the archive must
#                       link that fork's libps5platform.a, libc++/libc++abi/libunwind, and use its
#                       --wrap/--defsym recipe (PS5_Vulkan tools/radv-link.sh).
#   - PS5_Mesa          github.com/mihawk-99/PS5_Mesa @ 47e9eb61 (Mesa 26.2.0, -Dradv-winsys=ps5)
#   - The PS5 OpenGL SDK's opengnm-psbc compiler is only needed for the older ps5vk driver,
#     not for RADV (RADV compiles SPIR-V with its own ACO on the console).
#
# Host packages (Ubuntu 24.04): meson >= 1.4 (uv tool install meson; apt has 1.3), ninja-build,
# llvm-18-dev libclang-18-dev libclc-18-dev llvm-spirv-18 libllvmspirvlib-18-dev (host mesa_clc),
# python3-mako python3-yaml glslang-tools bison flex rsync, and lld >= 21 for linking titles
# (apt.llvm.org llvm-toolchain-noble-21: lld-21). lld 18/20 do not know
# -z nodynamic-undefined-weak; without it Mesa's weak dispatch references (e.g.
# radv_EnumeratePhysicalDevices) become dynamic imports and ps5-native-tool refuses the ELF.
set -euo pipefail

opt=${PS5VK_OPT:-$HOME/opt}
ps5vk=${PS5VK:-$opt/PS5_Vulkan}
variant=${1:-release}
sdk_rev=b83202be73e930050e00de0f1b4d9ec46c0391bf
mesa_rev=47e9eb61ee7e1112194557e1e047178562827bc6

clone() { # url dir rev
    [[ -d $2/.git ]] || git clone "$1" "$2"
    git -C "$2" cat-file -e "$3^{commit}" 2>/dev/null || git -C "$2" fetch origin
}
[[ -d $ps5vk/.git ]] || git clone https://github.com/mihawk-99/PS5_Vulkan "$ps5vk"
clone https://github.com/mihawk-99/PS5_PayloadSDK "$opt/PS5_PayloadSDK" "$sdk_rev"
clone https://github.com/mihawk-99/PS5_Mesa "$opt/PS5_Mesa" "$mesa_rev"
grep -q "sdk_revision=$sdk_rev" "$ps5vk/tools/setup-native-dependencies.sh" ||
    echo "warning: PS5_Vulkan now pins another payload SDK revision than $sdk_rev" >&2
grep -q "mesa_revision=$mesa_rev" "$ps5vk/tools/build-radv.sh" ||
    echo "warning: PS5_Vulkan now pins another Mesa revision than $mesa_rev" >&2

export PS5_PAYLOAD_SDK_FORK=$opt/PS5_PayloadSDK PS5_MESA_FORK=$opt/PS5_Mesa
export MESON=${MESON:-$([[ -x $HOME/.local/bin/meson ]] && echo "$HOME/.local/bin/meson" || command -v meson)}

# 1. The SDK fork (v0.42 zip + platform layer) and zlib, into $ps5vk/.deps/native.
bash "$ps5vk/tools/setup-native-dependencies.sh"
# 2. RADV for the console (cross build with prospero-clang; host mesa_clc first). ~10 min.
bash "$ps5vk/tools/build-radv.sh" "$variant"
# 3. Title tools: ps5-native-tool and the clean-room libc.prx (`make` builds them before the
#    root app; the root app's own link may fail on an unrelated ps5_fp_ieee import, ignored).
make -C "$ps5vk" app || true
[[ -x $ps5vk/build/host/ps5-native-tool && -f $ps5vk/runtime/libc.prx ]] || {
    echo "ps5-native-tool / libc.prx were not built" >&2
    exit 1
}
# 4. The project's RADV smoke-test title, linked with lld 21 (see header).
lld21=/usr/lib/llvm-21/bin
[[ -x $lld21/ld.lld ]] || { echo "lld 21 is needed at $lld21 (apt.llvm.org lld-21)" >&2; exit 2; }
shim=$opt/ps5vk-tools/llvm-config-lld21
mkdir -p "$(dirname "$shim")"
printf '%s\n' '#!/bin/sh' "# prospero-lld finds ld.lld via llvm-config --bindir: LLVM 21's lld, all else LLVM 18." \
    "if [ \"\$1\" = \"--bindir\" ]; then echo $lld21; else exec llvm-config-18 \"\$@\"; fi" > "$shim"
chmod +x "$shim"
archive=$ps5vk/.deps/native/radv-$variant/lib/libvulkan_radeon.ps5.a
[[ $variant == debug ]] && archive=$ps5vk/.deps/native/radv/lib/libvulkan_radeon.ps5.a
sed -e 's/--eh-frame-hdr "\${radv_link_flags\[@\]}"/--eh-frame-hdr -z nodynamic-undefined-weak "${radv_link_flags[@]}"/' \
    -e "s|^\"\$sdk_root/bin/prospero-lld\" \"\${radv_linker_script|LLVM_CONFIG=$shim \"\$sdk_root/bin/prospero-lld\" \"\${radv_linker_script|" \
    "$ps5vk/tools/build-radv-title.sh" > "$ps5vk/tools/build-radv-title-lld21.sh"
RADV_ARCHIVE=$archive bash "$ps5vk/tools/build-radv-title-lld21.sh"
echo "driver: $archive"
