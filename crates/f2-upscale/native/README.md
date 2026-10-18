# Native headers

Headers only (no libraries, no DLLs): the runtimes are loaded at run time from the player's
`upscalers/` folder (see `docs/UPSCALING.md`).

| Folder | What | From | License |
|---|---|---|---|
| `streamline/` | NVIDIA Streamline SDK 2.12 headers (the subset `f2_upscale.cpp` includes; `sl_nvperf.h` is not included) | <https://github.com/NVIDIA-RTX/Streamline> | MIT, `streamline/LICENSE.txt` |
| `ffx/` | AMD FidelityFX / FSR SDK 2.3.0 API headers (`Kits/FidelityFX/{api,upscalers,framegeneration}/include`, same layout) | <https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK> tag `v2.3.0` | MIT, `ffx/LICENSE.txt` |

Streamline accepts structures of older SDK versions, so these 2.12 headers work with newer
runtimes (the 2.13 one in the DLSS 310.x package).
