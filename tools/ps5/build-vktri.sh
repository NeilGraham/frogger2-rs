#!/usr/bin/env bash
# Builds tools/ps5/vktri (a minimal Vulkan triangle on VK_KHR_display) as a PS5 title folder
# linked with the PS5 RADV archive, the same way PS5_Vulkan links its titles
# (tools/build-radv-title.sh + tools/radv-link.sh). Run tools/ps5/build-vulkan-driver.sh first.
#
#   tools/ps5/build-vktri.sh [out-dir]     # default: $PS5VK/dist/F2VK00001
#
# Nothing is written into this repository (objects go to $PS5VK/build/f2-vktri).
set -euo pipefail

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
opt=${PS5VK_OPT:-$HOME/opt}
root=${PS5VK:-$opt/PS5_Vulkan}
sdk_root=$root/.deps/native/ps5-payload-sdk
archive=${RADV_ARCHIVE:-$root/.deps/native/radv-release/lib/libvulkan_radeon.ps5.a}
vkinc=$root/.deps/native/radv-release/include
native=$root/tooling/native
tool=$root/build/host/ps5-native-tool
work=$root/build/f2-vktri
title_id=F2VK00001
app=${1:-$root/dist/$title_id}
shim=$opt/ps5vk-tools/llvm-config-lld21
for f in "$archive" "$tool" "$sdk_root/bin/prospero-lld" "$shim" "$root/runtime/libc.prx"; do
    [[ -e $f ]] || { echo "missing $f (run tools/ps5/build-vulkan-driver.sh)" >&2; exit 2; }
done

mkdir -p "$work/obj" "$work/stubs" "$work/gen"
cc() { PS5_PAYLOAD_SDK="$sdk_root" sh "$root/tooling/prospero-clang18" "$@"; }
glslangValidator -V --target-env vulkan1.0 --vn tri_vert "$here/vktri/tri.vert" -o "$work/gen/tri_vert.h" > /dev/null
glslangValidator -V --target-env vulkan1.0 --vn tri_frag "$here/vktri/tri.frag" -o "$work/gen/tri_frag.h" > /dev/null
cc -std=c11 -O2 -Wall -Wextra -I "$vkinc" -I "$work/gen" -c "$here/vktri/vktri.c" -o "$work/obj/vktri.o"
cc -std=c++20 -O2 -Wall -fno-exceptions -fno-rtti -c "$native/app_crt.cpp" -o "$work/obj/app_crt.o"
cc -std=c++20 -O2 -Wall -fno-exceptions -fno-rtti -c "$native/app_cpp_runtime.cpp" -o "$work/obj/app_cpp_runtime.o"
extra_objs=() extra_flags=()
if [[ ${KYTY_SHIM:-0} == 1 ]]; then # emulator-only wrappers (vktri/kyty_shim.c)
    cc -std=c11 -O2 -Wall -c "$here/vktri/kyty_shim.c" -o "$work/obj/kyty_shim.o"
    # In an archive so --exclude-libs=ALL keeps the __wrap_ symbols local (the title
    # converter refuses exports).
    rm -f "$work/obj/libkytyshim.a"
    "$sdk_root/bin/prospero-ar" rcs "$work/obj/libkytyshim.a" "$work/obj/kyty_shim.o"
    extra_objs=("$work/obj/libkytyshim.a")
    extra_flags=(--wrap=getenv --wrap=setenv --wrap=strdup --wrap=strndup)
fi
stub() {
    cc -std=c11 -O2 -fPIC -c "$root/$2" -o "$work/obj/$1_stub.o"
    "$sdk_root/bin/prospero-lld" --shared -soname "$1.prx" -o "$work/stubs/$1.so" "$work/obj/$1_stub.o"
}
stub libSceAgc vendor/ps5/sdk/stubs/agc_canary_link_stub.c
stub libSceAgcDriver vendor/ps5/sdk/stubs/agc_driver_canary_link_stub.c

# shellcheck source=/dev/null
source "$root/tools/radv-link.sh"
radv_link_recipe "$root" "$sdk_root" "$archive" || exit 2
LLVM_CONFIG=$shim "$sdk_root/bin/prospero-lld" "${radv_linker_script[@]}" --eh-frame-hdr \
    -z nodynamic-undefined-weak "${radv_link_flags[@]}" ${extra_flags[@]+"${extra_flags[@]}"} \
    --version-script "$native/app-symbols.map" --exclude-libs=ALL -e _start -o "$work/llvm-pie.elf" \
    "$work/obj/app_crt.o" "$work/obj/app_cpp_runtime.o" "$work/obj/vktri.o" ${extra_objs[@]+"${extra_objs[@]}"} \
    "$work/stubs/libSceAgc.so" "$work/stubs/libSceAgcDriver.so" "${radv_link_inputs[@]}" \
    --as-needed "$sdk_root"/target/lib/*.so
"$tool" link --in "$work/llvm-pie.elf" --out "$work/eboot.elf" --stub-dir "$sdk_root/target/lib" \
    --stub "$work/stubs/libSceAgc.so" --stub "$work/stubs/libSceAgcDriver.so" \
    --module-sdk 0x02000009 --companion-sdk 0x08050001 --file-name eboot.elf

rm -rf -- "$app"
mkdir -p "$app/sce_sys" "$app/sce_module"
"$tool" self --sign --in "$work/eboot.elf" --out "$app/eboot.bin" --magic 0x1D3D154F
python3 -I - "$root/sce_sys/param-radv.json" "$app/sce_sys/param.json" "$title_id" <<'PY'
import json, sys
p = json.load(open(sys.argv[1]))
tid = sys.argv[3]
p["titleId"] = tid
p["conceptId"] = tid[4:]
p["contentId"] = f"UP9000-{tid}_00-F2VKTRIANGLE0001"
p["localizedParameters"]["en-US"]["titleName"] = "F2 Vulkan triangle"
json.dump(p, open(sys.argv[2], "w"), indent=2)
PY
for asset in icon0.png pic0.dds pic1.dds; do
    [[ -f $root/sce_sys/$asset ]] && cp "$root/sce_sys/$asset" "$app/sce_sys/"
done
cp "$root/runtime/libc.prx" "$app/sce_module/libc.prx"
"$tool" self --inspect --file "$app/eboot.bin" > /dev/null
echo "vktri title: $app ($(stat -c %s "$app/eboot.bin") bytes)"
