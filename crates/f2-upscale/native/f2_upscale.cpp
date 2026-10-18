// f2_upscale: see f2_upscale.h. DLSS super resolution, DLSS frame generation (DLSS-G / multi
// frame generation) and Reflex through NVIDIA Streamline in manual-hooking mode; FSR 3.1 / FSR 4
// upscaling and FSR frame generation through the AMD FidelityFX API loader. Every library is
// loaded at run time from the runtime folder; nothing here links against them.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_6.h>
#include <wrl/client.h>
#include <cstdio>
#include <cstring>
#include <cstdarg>
#include <string>
#include <vector>

#include <sl.h>
#include <sl_consts.h>
#include <sl_dlss.h>
#include <sl_dlss_g.h>
#include <sl_pcl.h>
#include <sl_reflex.h>

#include "ffx/api/include/ffx_api.h"
#include "ffx/api/include/ffx_api_loader.h"
#include "ffx/api/include/dx12/ffx_api_dx12.h"
#include "ffx/upscalers/include/ffx_upscale.h"
#include "ffx/framegeneration/include/ffx_framegeneration.h"
#include "ffx/framegeneration/include/dx12/ffx_api_framegeneration_dx12.h"

#include "f2_upscale.h"

using Microsoft::WRL::ComPtr;

namespace {

constexpr uint32_t kSlots = 6;       // command lists in flight
constexpr uint32_t kBuffers = 3;     // swapchain buffers
// Identifies the game to NGX (any fixed GUID; NVIDIA's guide asks for one per project).
constexpr const char* kProjectId = "4f1e2b7a-93c5-4d0e-b8a1-6c2f9e5d3a70";
constexpr UINT kReadState = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;

f2up_log_fn g_log = nullptr;
PfnFfxDispatch g_ffx_dispatch = nullptr; // for the frame generation callback

void logf(int level, const char* fmt, ...) {
    if (!g_log) return;
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    g_log(level, buf);
}

void sl_log(sl::LogType type, const char* msg) {
    if (type == sl::LogType::eError) logf(2, "[streamline] %s", msg);
    else if (type == sl::LogType::eWarn) logf(1, "[streamline] %s", msg);
}

void ffx_log(uint32_t type, const wchar_t* msg) {
    char buf[1024];
    WideCharToMultiByte(CP_UTF8, 0, msg, -1, buf, sizeof buf, nullptr, nullptr);
    logf(type == FFX_API_MESSAGE_TYPE_ERROR ? 2 : 1, "[fsr] %s", buf);
}

void barrier(ID3D12GraphicsCommandList* l, ID3D12Resource* r, D3D12_RESOURCE_STATES from, D3D12_RESOURCE_STATES to) {
    if (from == to) return;
    D3D12_RESOURCE_BARRIER b{};
    b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    b.Transition.pResource = r;
    b.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
    b.Transition.StateBefore = from;
    b.Transition.StateAfter = to;
    l->ResourceBarrier(1, &b);
}

void copy4x4(const float* m, sl::float4x4& o) {
    for (int r = 0; r < 4; ++r) o.row[r] = sl::float4(m[r * 4 + 0], m[r * 4 + 1], m[r * 4 + 2], m[r * 4 + 3]);
}

template <typename T> bool proc(HMODULE m, T*& fn, const char* name) {
    fn = reinterpret_cast<T*>(GetProcAddress(m, name));
    if (!fn) logf(2, "%s not exported", name);
    return fn != nullptr;
}

FfxApiResource ffx_res(ID3D12Resource* r, uint32_t state, uint32_t format) {
    FfxApiResource res = ffxApiGetResourceDX12(r, state);
    // wgpu may create a resource typeless; the format the view is read with.
    if (format) res.description.format = format;
    return res;
}

} // namespace

struct f2up {
    ComPtr<ID3D12Device> dev;
    ComPtr<ID3D12CommandQueue> queue;
    HWND hwnd = nullptr;
    std::wstring dir, logdir;
    f2up_options_t opt{};
    uint32_t sr = F2UP_SR_NONE, fg = F2UP_FG_NONE;

    // Command lists.
    struct Slot {
        ComPtr<ID3D12CommandAllocator> alloc;
        ComPtr<ID3D12GraphicsCommandList> list;
        uint64_t fence = 0;
    } slots[kSlots];
    uint32_t slot = 0;
    ComPtr<ID3D12Fence> fence;
    uint64_t fence_value = 0;
    HANDLE fence_event = nullptr;

    // Swapchain.
    ComPtr<IDXGIFactory2> factory;
    ComPtr<IDXGISwapChain3> sc;
    uint32_t width = 0, height = 0;
    bool tearing = false;

    // Streamline.
    HMODULE sl = nullptr;
    bool sl_ok = false, dlss_ok = false, dlssg_ok = false, reflex_ok = false, sc_proxied = false;
    PFun_slInit* slInit = nullptr;
    PFun_slShutdown* slShutdown = nullptr;
    PFun_slSetD3DDevice* slSetD3DDevice = nullptr;
    PFun_slUpgradeInterface* slUpgradeInterface = nullptr;
    PFun_slIsFeatureSupported* slIsFeatureSupported = nullptr;
    PFun_slGetFeatureFunction* slGetFeatureFunction = nullptr;
    PFun_slGetNewFrameToken* slGetNewFrameToken = nullptr;
    PFun_slSetConstants* slSetConstants = nullptr;
    PFun_slSetTagForFrame* slSetTagForFrame = nullptr;
    PFun_slEvaluateFeature* slEvaluateFeature = nullptr;
    PFun_slGetFeatureVersion* slGetFeatureVersion = nullptr;
    PFun_slFreeResources* slFreeResources = nullptr;
    PFun_slDLSSGetOptimalSettings* slDLSSGetOptimalSettings = nullptr;
    PFun_slDLSSSetOptions* slDLSSSetOptions = nullptr;
    PFun_slDLSSGSetOptions* slDLSSGSetOptions = nullptr;
    PFun_slDLSSGGetState* slDLSSGGetState = nullptr;
    PFun_slReflexSetOptions* slReflexSetOptions = nullptr;
    PFun_slReflexSleep* slReflexSleep = nullptr;
    PFun_slPCLSetMarker* slPCLSetMarker = nullptr;
    sl::FrameToken* token = nullptr;
    uint64_t token_frame = UINT64_MAX;
    bool token_presented = true;
    sl::ViewportHandle viewport{0u};
    sl::DLSSOptions dlss_last{};
    bool dlss_set = false;
    int dlssg_last_mode = -1;
    uint32_t dlssg_last_frames = 0, dlssg_last_sizes[4] = {};
    float dlssg_last_target = -1;
    uint32_t dlssg_max = 1;
    bool dlssg_dynamic = false;
    uint32_t dlssg_presented = 0;
    ULONGLONG last_poll = 0;
    std::string dlss_version, dlssg_version;

    // FidelityFX.
    HMODULE ffxm = nullptr;
    ffxFunctions fx{};
    bool ffx_ok = false;
    ffxContext up_ctx = nullptr, fg_ctx = nullptr, sc_ctx = nullptr;
    uint32_t up_ctx_sr = 0, up_ctx_w = 0, up_ctx_h = 0;
    uint32_t fg_ctx_w = 0, fg_ctx_h = 0;
    uint64_t fsr3_id = 0, fsr4_id = 0, fg3_id = 0, fg4_id = 0;
    std::string fsr3_name, fsr4_name, fg3_name, fg4_name, up_name, fg_name;
    ComPtr<ID3D12Resource> hudless[2];
    uint32_t hudless_w = 0, hudless_h = 0;
    uint64_t fg_frame_id = 0;
    bool fsr_fg_ok = false;

    uint32_t sr_active = 0;
    char error[256] = {};
    ComPtr<ID3D12Device> warp; // DLSS 5 helper
    bool nr = false;           // f2up_init_t::nr_helpers
    uint64_t presents = 0;

    void fail(const char* fmt, ...) {
        va_list ap;
        va_start(ap, fmt);
        vsnprintf(error, sizeof error, fmt, ap);
        va_end(ap);
        logf(2, "%s", error);
    }

    // ------------------------------------------------------------------ command lists
    ID3D12GraphicsCommandList* begin_list() {
        Slot& s = slots[slot];
        wait(s.fence);
        s.alloc->Reset();
        s.list->Reset(s.alloc.Get(), nullptr);
        return s.list.Get();
    }
    void submit_list() {
        Slot& s = slots[slot];
        s.list->Close();
        ID3D12CommandList* l[] = { s.list.Get() };
        queue->ExecuteCommandLists(1, l);
        s.fence = ++fence_value;
        queue->Signal(fence.Get(), s.fence);
        slot = (slot + 1) % kSlots;
    }
    void wait(uint64_t v) {
        if (v == 0 || fence->GetCompletedValue() >= v) return;
        fence->SetEventOnCompletion(v, fence_event);
        WaitForSingleObject(fence_event, INFINITE);
    }
    void wait_idle() {
        queue->Signal(fence.Get(), ++fence_value);
        wait(fence_value);
    }

    bool init_lists() {
        for (auto& s : slots) {
            if (FAILED(dev->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&s.alloc)))) return false;
            if (FAILED(dev->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, s.alloc.Get(), nullptr, IID_PPV_ARGS(&s.list)))) return false;
            s.list->Close();
        }
        if (FAILED(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)))) return false;
        fence_event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        return fence_event != nullptr;
    }

    // ------------------------------------------------------------------ Streamline
    bool load_sl(bool want_dlss, bool want_dlssg) {
        if (sl_ok) return true;
        std::wstring p = dir + L"\\sl.interposer.dll";
        sl = LoadLibraryW(p.c_str());
        if (!sl) {
            fail("Streamline not found (%ls): put the DLSS / Streamline DLLs in the upscalers folder", p.c_str());
            return false;
        }
        bool ok = proc(sl, slInit, "slInit") & proc(sl, slShutdown, "slShutdown") & proc(sl, slSetD3DDevice, "slSetD3DDevice") &
                  proc(sl, slUpgradeInterface, "slUpgradeInterface") & proc(sl, slIsFeatureSupported, "slIsFeatureSupported") &
                  proc(sl, slGetFeatureFunction, "slGetFeatureFunction") & proc(sl, slGetNewFrameToken, "slGetNewFrameToken") &
                  proc(sl, slSetConstants, "slSetConstants") & proc(sl, slSetTagForFrame, "slSetTagForFrame") &
                  proc(sl, slEvaluateFeature, "slEvaluateFeature") & proc(sl, slGetFeatureVersion, "slGetFeatureVersion") &
                  proc(sl, slFreeResources, "slFreeResources");
        if (!ok) {
            fail("sl.interposer.dll is missing exports");
            return false;
        }
        static const wchar_t* paths[1];
        static std::wstring dir_s;
        dir_s = dir;
        paths[0] = dir_s.c_str();
        std::vector<sl::Feature> feats = { sl::kFeatureDLSS, sl::kFeatureReflex, sl::kFeaturePCL };
        if (want_dlssg) feats.push_back(sl::kFeatureDLSS_G);
        (void)want_dlss;
        static std::vector<sl::Feature> feats_s;
        feats_s = feats;
        sl::Preferences pref;
        pref.showConsole = false;
        pref.logLevel = sl::LogLevel::eDefault;
        pref.pathsToPlugins = paths;
        pref.numPathsToPlugins = 1;
        static std::wstring logdir_s;
        logdir_s = logdir;
        pref.pathToLogsAndData = logdir_s.empty() ? nullptr : logdir_s.c_str();
        pref.logMessageCallback = sl_log;
        pref.flags = sl::PreferenceFlags::eUseManualHooking | sl::PreferenceFlags::eDisableCLStateTracking | sl::PreferenceFlags::eUseFrameBasedResourceTagging;
        pref.featuresToLoad = feats_s.data();
        pref.numFeaturesToLoad = (uint32_t)feats_s.size();
        pref.applicationId = 0;
        pref.engine = sl::EngineType::eCustom;
        pref.engineVersion = "f2-engine";
        pref.projectId = kProjectId;
        pref.renderAPI = sl::RenderAPI::eD3D12;
        sl::Result r = slInit(pref, sl::kSDKVersion);
        if (r != sl::Result::eOk) {
            fail("slInit failed (%d)", (int)r);
            return false;
        }
        r = slSetD3DDevice(dev.Get());
        if (r != sl::Result::eOk) {
            fail("slSetD3DDevice failed (%d)", (int)r);
            return false;
        }
        sl_ok = true;
        LUID luid = dev->GetAdapterLuid();
        sl::AdapterInfo ai;
        ai.deviceLUID = reinterpret_cast<uint8_t*>(&luid);
        ai.deviceLUIDSizeInBytes = sizeof luid;
        auto supported = [&](sl::Feature f, const char* name) {
            sl::Result s = slIsFeatureSupported(f, ai);
            if (s != sl::Result::eOk) logf(1, "%s not supported on this GPU / driver (%d)", name, (int)s);
            return s == sl::Result::eOk;
        };
        dlss_ok = supported(sl::kFeatureDLSS, "DLSS");
        reflex_ok = supported(sl::kFeatureReflex, "Reflex");
        dlssg_ok = want_dlssg && supported(sl::kFeatureDLSS_G, "DLSS Frame Generation");
        auto fn = [&](sl::Feature f, const char* name, void** out) {
            void* p = nullptr;
            if (slGetFeatureFunction(f, name, p) != sl::Result::eOk) p = nullptr;
            *out = p;
            return p != nullptr;
        };
        if (dlss_ok) {
            dlss_ok = fn(sl::kFeatureDLSS, "slDLSSGetOptimalSettings", (void**)&slDLSSGetOptimalSettings) &&
                      fn(sl::kFeatureDLSS, "slDLSSSetOptions", (void**)&slDLSSSetOptions);
        }
        if (dlssg_ok) {
            dlssg_ok = fn(sl::kFeatureDLSS_G, "slDLSSGSetOptions", (void**)&slDLSSGSetOptions) &&
                       fn(sl::kFeatureDLSS_G, "slDLSSGGetState", (void**)&slDLSSGGetState);
        }
        if (reflex_ok) {
            reflex_ok = fn(sl::kFeatureReflex, "slReflexSetOptions", (void**)&slReflexSetOptions) &&
                        fn(sl::kFeatureReflex, "slReflexSleep", (void**)&slReflexSleep);
        }
        fn(sl::kFeaturePCL, "slPCLSetMarker", (void**)&slPCLSetMarker);
        auto version = [&](sl::Feature f) {
            sl::FeatureVersion v;
            if (slGetFeatureVersion(f, v) != sl::Result::eOk) return std::string();
            char b[64];
            snprintf(b, sizeof b, "%u.%u.%u", v.versionNGX.major, v.versionNGX.minor, v.versionNGX.build);
            return std::string(b);
        };
        dlss_version = version(sl::kFeatureDLSS);
        if (dlssg_ok) dlssg_version = version(sl::kFeatureDLSS_G);
        logf(0, "Streamline loaded: DLSS %s%s, DLSS-G %s, Reflex %s", dlss_ok ? "yes " : "no", dlss_version.c_str(),
             dlssg_ok ? dlssg_version.c_str() : "no", reflex_ok ? "yes" : "no");
        apply_reflex();
        return true;
    }

    void apply_reflex() {
        if (!reflex_ok || !slReflexSetOptions) return;
        sl::ReflexOptions ro;
        // Frame generation needs Reflex on.
        uint32_t want = opt.reflex == 0 && fg == F2UP_FG_DLSS && opt.fg_mode != 0 ? 1 : opt.reflex;
        ro.mode = want == 0 ? sl::ReflexMode::eOff : (want == 2 ? sl::ReflexMode::eLowLatencyWithBoost : sl::ReflexMode::eLowLatency);
        ro.useMarkersToOptimize = true;
        if (slReflexSetOptions(ro) != sl::Result::eOk) logf(1, "slReflexSetOptions failed");
    }

    static sl::DLSSMode dlss_mode(uint32_t m) {
        switch (m) {
        case 0: return sl::DLSSMode::eDLAA;
        case 1: return sl::DLSSMode::eMaxQuality;
        case 2: return sl::DLSSMode::eBalanced;
        case 3: return sl::DLSSMode::eMaxPerformance;
        default: return sl::DLSSMode::eUltraPerformance;
        }
    }

    sl::DLSSOptions dlss_options(uint32_t ow, uint32_t oh) const {
        sl::DLSSOptions o;
        o.mode = dlss_mode(opt.sr_mode);
        o.outputWidth = ow;
        o.outputHeight = oh;
        o.colorBuffersHDR = sl::Boolean::eFalse;
        auto p = (sl::DLSSPreset)opt.dlss_preset;
        o.dlaaPreset = o.qualityPreset = o.balancedPreset = o.performancePreset = o.ultraPerformancePreset = o.ultraQualityPreset = p;
        return o;
    }

    void ensure_token(uint64_t frame) {
        if (!sl_ok || token_frame == frame) return;
        uint32_t idx = (uint32_t)frame;
        sl::FrameToken* t = nullptr;
        if (slGetNewFrameToken(t, &idx) == sl::Result::eOk) {
            token = t;
            token_frame = frame;
            token_presented = false;
        }
    }

    void marker(sl::PCLMarker m) {
        if (slPCLSetMarker && token) slPCLSetMarker(m, *token);
    }

    void apply_dlssg(uint32_t rw, uint32_t rh, uint32_t ow, uint32_t oh) {
        if (!dlssg_ok || !sc_proxied) return;
        sl::DLSSGOptions g;
        uint32_t frames = 1;
        if (opt.fg_mode == 0) {
            g.mode = sl::DLSSGMode::eOff;
        } else if (opt.fg_mode == 4) {
            if (dlssg_dynamic) {
                g.mode = sl::DLSSGMode::eDynamic;
                g.dynamicTargetFrameRate = opt.fg_target_fps;
                frames = dlssg_max;
            } else {
                g.mode = sl::DLSSGMode::eOn;
                frames = dlssg_max;
            }
        } else {
            g.mode = sl::DLSSGMode::eOn;
            frames = opt.fg_mode;
        }
        if (dlssg_max && frames > dlssg_max) frames = dlssg_max;
        if (frames < 1) frames = 1;
        g.numFramesToGenerate = frames;
        g.mvecDepthWidth = rw;
        g.mvecDepthHeight = rh;
        g.colorWidth = ow;
        g.colorHeight = oh;
        g.numBackBuffers = kBuffers;
        uint32_t sizes[4] = { rw, rh, ow, oh };
        if ((int)g.mode == dlssg_last_mode && frames == dlssg_last_frames && g.dynamicTargetFrameRate == dlssg_last_target &&
            memcmp(sizes, dlssg_last_sizes, sizeof sizes) == 0)
            return;
        sl::Result r = slDLSSGSetOptions(viewport, g);
        if (r != sl::Result::eOk) {
            fail("slDLSSGSetOptions failed (%d)", (int)r);
            return;
        }
        logf(0, "DLSS-G: %s, %u generated frame(s), target %.0f fps (render %ux%u, output %ux%u)",
             g.mode == sl::DLSSGMode::eOff ? "off" : g.mode == sl::DLSSGMode::eDynamic ? "dynamic" : "on", frames, g.dynamicTargetFrameRate, rw, rh, ow, oh);
        dlssg_last_mode = (int)g.mode;
        dlssg_last_frames = frames;
        dlssg_last_target = g.dynamicTargetFrameRate;
        memcpy(dlssg_last_sizes, sizes, sizeof sizes);
    }

    void poll_dlssg() {
        if (!dlssg_ok || !sc_proxied || !slDLSSGGetState) return;
        ULONGLONG now = GetTickCount64();
        if (now - last_poll < 1000) return;
        last_poll = now;
        sl::DLSSGState st;
        if (slDLSSGGetState(viewport, st, nullptr) == sl::Result::eOk) {
            uint32_t mx = st.numFramesToGenerateMax ? st.numFramesToGenerateMax : 1;
            bool dyn = st.bIsDynamicMFGSupported == sl::Boolean::eTrue;
            if (mx != dlssg_max || dyn != dlssg_dynamic) {
                dlssg_max = mx;
                dlssg_dynamic = dyn;
                dlssg_last_mode = -1; // re-apply with the new limits
                logf(0, "DLSS-G: up to %u generated frame(s) per frame, dynamic %s", mx, dyn ? "supported" : "not supported");
            }
            dlssg_presented = st.numFramesActuallyPresented;
        }
    }

    // ------------------------------------------------------------------ FidelityFX
    bool load_ffx() {
        if (ffx_ok) return true;
        std::wstring p = dir + L"\\amd_fidelityfx_loader_dx12.dll";
        // The loader loads the effect DLLs (upscaler, frame generation) by name.
        SetDllDirectoryW(dir.c_str());
        ffxm = LoadLibraryExW(p.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
        if (!ffxm) {
            fail("FSR not found (%ls): put the AMD FidelityFX DLLs in the upscalers folder", p.c_str());
            return false;
        }
        ffxLoadFunctions(&fx, ffxm);
        if (!fx.CreateContext || !fx.DestroyContext || !fx.Configure || !fx.Query || !fx.Dispatch) {
            fail("amd_fidelityfx_loader_dx12.dll is missing exports");
            return false;
        }
        g_ffx_dispatch = fx.Dispatch;
        ffx_ok = true;
        ffxConfigureDescGlobalDebug1 dbg{};
        dbg.header.type = FFX_API_CONFIGURE_DESC_TYPE_GLOBALDEBUG1;
        dbg.fpMessage = ffx_log;
        dbg.debugLevel = FFX_API_CONFIGURE_GLOBALDEBUG_LEVEL_WARNINGS;
        fx.Configure(nullptr, &dbg.header);
        query_versions(FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE, fsr3_id, fsr3_name, fsr4_id, fsr4_name, "FSR upscaling");
        query_versions(FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATION, fg3_id, fg3_name, fg4_id, fg4_name, "FSR frame generation");
        fsr_fg_ok = fg3_id || fg4_id;
        if (getenv("F2_FSR4_PROBE")) probe_fsr4();
        return true;
    }

    // Testing (F2_FSR4_PROBE=1): the providers the DLL has without the GPU filter, and an FSR 4
    // context created on this device regardless (what AMD's code says on an unsupported GPU).
    void probe_fsr4() {
        ffxQueryDescGetVersions q{};
        q.header.type = FFX_API_QUERY_DESC_TYPE_GET_VERSIONS;
        q.createDescType = FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE;
        q.device = nullptr;
        uint64_t count = 0;
        q.outputCount = &count;
        ffxReturnCode_t r = fx.Query(nullptr, &q.header);
        logf(0, "FSR4 probe: unfiltered query -> %u, %llu versions", r, (unsigned long long)count);
        if (r != FFX_API_RETURN_OK || !count) return;
        std::vector<uint64_t> ids(count);
        std::vector<const char*> names(count);
        q.versionIds = ids.data();
        q.versionNames = names.data();
        if (fx.Query(nullptr, &q.header) != FFX_API_RETURN_OK) return;
        // 4.1.1 in the id layout the listed ones use ((major << 22) | (minor << 12) | patch).
        bool listed4 = false;
        for (uint64_t i = 0; i < count; ++i) listed4 |= names[i] && names[i][0] == '4';
        if (!listed4) {
            ids.push_back((ids[0] & ~0xffffffffull) | ((4u << 22) | (1u << 12) | 1u));
            names.push_back("4.1.1 (not listed, id derived)");
            ++count;
        }
        for (uint64_t i = 0; i < count; ++i) {
            std::string n = names[i] ? names[i] : "?";
            logf(0, "FSR4 probe: version %s id 0x%llx", n.c_str(), (unsigned long long)ids[i]);
            if (n[0] != '4') continue;
            ffxCreateContextDescUpscale c{};
            c.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE;
            c.flags = FFX_UPSCALE_ENABLE_DEPTH_INVERTED | FFX_UPSCALE_ENABLE_DEPTH_INFINITE | FFX_UPSCALE_ENABLE_DYNAMIC_RESOLUTION | FFX_UPSCALE_ENABLE_NON_LINEAR_COLORSPACE;
            c.maxRenderSize = { width, height };
            c.maxUpscaleSize = { width, height };
            c.fpMessage = ffx_log;
            ffxCreateContextDescUpscaleVersion v{};
            v.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE_VERSION;
            v.version = FFX_UPSCALER_VERSION;
            ffxCreateBackendDX12Desc b{};
            b.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_DX12;
            b.device = dev.Get();
            ffxOverrideVersion ov{};
            ov.header.type = FFX_API_DESC_TYPE_OVERRIDE_VERSION;
            ov.versionId = ids[i];
            c.header.pNext = &v.header;
            v.header.pNext = &b.header;
            b.header.pNext = &ov.header;
            ffxContext ctx = nullptr;
            ffxReturnCode_t cr = fx.CreateContext(&ctx, &c.header, nullptr);
            logf(0, "FSR4 probe: create FSR %s on this GPU -> %u", n.c_str(), cr);
            if (cr == FFX_API_RETURN_OK && ctx) {
                ffxQueryGetProviderVersion pv{};
                pv.header.type = FFX_API_QUERY_DESC_TYPE_GET_PROVIDER_VERSION;
                if (fx.Query(&ctx, &pv.header) == FFX_API_RETURN_OK && pv.versionName) logf(0, "FSR4 probe: context provider %s", pv.versionName);
                fx.DestroyContext(&ctx, nullptr);
            }
        }
    }

    // The newest 3.x and 4.x providers of an effect.
    void query_versions(uint64_t type, uint64_t& id3, std::string& n3, uint64_t& id4, std::string& n4, const char* what) {
        ffxQueryDescGetVersions q{};
        q.header.type = FFX_API_QUERY_DESC_TYPE_GET_VERSIONS;
        q.createDescType = type;
        q.device = dev.Get();
        uint64_t count = 0;
        q.outputCount = &count;
        if (fx.Query(nullptr, &q.header) != FFX_API_RETURN_OK || count == 0) {
            logf(1, "%s: no versions available", what);
            return;
        }
        std::vector<uint64_t> ids(count);
        std::vector<const char*> names(count);
        q.versionIds = ids.data();
        q.versionNames = names.data();
        if (fx.Query(nullptr, &q.header) != FFX_API_RETURN_OK) return;
        std::string list;
        // The list is newest first; keep the first of each major version.
        for (uint64_t i = 0; i < count; ++i) {
            std::string n = names[i] ? names[i] : "";
            list += (i ? ", " : "") + n;
            size_t d = n.find_first_of("0123456789");
            char major = d == std::string::npos ? 0 : n[d];
            if (major == '3' && !id3) {
                id3 = ids[i];
                n3 = n;
            } else if (major >= '4' && !id4) {
                id4 = ids[i];
                n4 = n;
            }
        }
        logf(0, "%s versions: %s", what, list.c_str());
    }

    void destroy_fsr_upscaler() {
        if (up_ctx) {
            fx.DestroyContext(&up_ctx, nullptr);
            up_ctx = nullptr;
        }
    }

    bool ensure_fsr_upscaler(uint32_t which, uint32_t ow, uint32_t oh) {
        if (up_ctx && up_ctx_sr == which && up_ctx_w == ow && up_ctx_h == oh) return true;
        if (up_ctx) wait_idle();
        destroy_fsr_upscaler();
        uint64_t id = which == F2UP_SR_FSR4 ? fsr4_id : fsr3_id;
        if (!id && which == F2UP_SR_FSR4 && fsr3_id) {
            // FSR 4 needs an AMD Radeon RX 7000 / 9000 (its driver provides it): FSR 3.1 instead.
            fail("FSR 4 is not available on this GPU (it needs an AMD Radeon RX 7000 / 9000): using FSR 3.1");
            id = fsr3_id;
        }
        if (!id) {
            fail("FSR is not available");
            return false;
        }
        ffxCreateContextDescUpscale c{};
        c.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE;
        c.flags = FFX_UPSCALE_ENABLE_DEPTH_INVERTED | FFX_UPSCALE_ENABLE_DEPTH_INFINITE | FFX_UPSCALE_ENABLE_DYNAMIC_RESOLUTION |
                  FFX_UPSCALE_ENABLE_NON_LINEAR_COLORSPACE;
        c.maxRenderSize = { ow, oh };
        c.maxUpscaleSize = { ow, oh };
        c.fpMessage = ffx_log;
        ffxCreateContextDescUpscaleVersion v{};
        v.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_UPSCALE_VERSION;
        v.version = FFX_UPSCALER_VERSION;
        ffxCreateBackendDX12Desc b{};
        b.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_DX12;
        b.device = dev.Get();
        ffxOverrideVersion ov{};
        ov.header.type = FFX_API_DESC_TYPE_OVERRIDE_VERSION;
        ov.versionId = id;
        c.header.pNext = &v.header;
        v.header.pNext = &b.header;
        b.header.pNext = &ov.header;
        ffxReturnCode_t r = fx.CreateContext(&up_ctx, &c.header, nullptr);
        if (r != FFX_API_RETURN_OK) {
            up_ctx = nullptr;
            fail("FSR upscaler creation failed (%u)", r);
            return false;
        }
        up_ctx_sr = which;
        up_ctx_w = ow;
        up_ctx_h = oh;
        ffxQueryGetProviderVersion pv{};
        pv.header.type = FFX_API_QUERY_DESC_TYPE_GET_PROVIDER_VERSION;
        up_name = "FSR";
        if (fx.Query(&up_ctx, &pv.header) == FFX_API_RETURN_OK && pv.versionName) up_name = std::string("FSR ") + pv.versionName;
        logf(0, "%s upscaler created (output %ux%u)", up_name.c_str(), ow, oh);
        return true;
    }

    void destroy_fsr_fg() {
        if (!fg_ctx) return;
        ffxConfigureDescFrameGeneration c{};
        c.header.type = FFX_API_CONFIGURE_DESC_TYPE_FRAMEGENERATION;
        c.swapChain = sc.Get();
        c.frameGenerationEnabled = false;
        fx.Configure(&fg_ctx, &c.header);
        fx.DestroyContext(&fg_ctx, nullptr);
        fg_ctx = nullptr;
    }

    bool ensure_fsr_fg(uint32_t ow, uint32_t oh) {
        if (fg_ctx && fg_ctx_w == ow && fg_ctx_h == oh) return true;
        destroy_fsr_fg();
        // The ML (4.x) generator with FSR 4 upscaling when the GPU has it, else 3.1.
        uint64_t id = (sr == F2UP_SR_FSR4 && fg4_id) ? fg4_id : (fg3_id ? fg3_id : fg4_id);
        ffxCreateContextDescFrameGeneration c{};
        c.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATION;
        c.flags = FFX_FRAMEGENERATION_ENABLE_DEPTH_INVERTED | FFX_FRAMEGENERATION_ENABLE_DEPTH_INFINITE;
        c.displaySize = { ow, oh };
        c.maxRenderSize = { ow, oh };
        c.backBufferFormat = FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM;
        ffxCreateContextDescFrameGenerationVersion v{};
        v.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATION_VERSION;
        v.version = FFX_FRAMEGENERATION_VERSION;
        ffxCreateContextDescFrameGenerationHudless h{};
        h.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATION_HUDLESS;
        h.hudlessBackBufferFormat = FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM;
        ffxCreateBackendDX12Desc b{};
        b.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_BACKEND_DX12;
        b.device = dev.Get();
        ffxOverrideVersion ov{};
        ov.header.type = FFX_API_DESC_TYPE_OVERRIDE_VERSION;
        ov.versionId = id;
        c.header.pNext = &v.header;
        v.header.pNext = &h.header;
        h.header.pNext = &b.header;
        if (id) b.header.pNext = &ov.header;
        ffxReturnCode_t r = fx.CreateContext(&fg_ctx, &c.header, nullptr);
        if (r != FFX_API_RETURN_OK) {
            fg_ctx = nullptr;
            fail("FSR frame generation creation failed (%u)", r);
            return false;
        }
        fg_ctx_w = ow;
        fg_ctx_h = oh;
        ffxQueryGetProviderVersion pv{};
        pv.header.type = FFX_API_QUERY_DESC_TYPE_GET_PROVIDER_VERSION;
        fg_name = "FSR Frame Generation";
        if (fx.Query(&fg_ctx, &pv.header) == FFX_API_RETURN_OK && pv.versionName) fg_name = std::string("FSR Frame Generation ") + pv.versionName;
        logf(0, "%s created (%ux%u)", fg_name.c_str(), ow, oh);
        return true;
    }

    bool ensure_hudless(uint32_t w, uint32_t h) {
        if (hudless[0] && hudless_w == w && hudless_h == h) return true;
        D3D12_HEAP_PROPERTIES hp{};
        hp.Type = D3D12_HEAP_TYPE_DEFAULT;
        D3D12_RESOURCE_DESC d{};
        d.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
        d.Width = w;
        d.Height = h;
        d.DepthOrArraySize = 1;
        d.MipLevels = 1;
        d.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        d.SampleDesc.Count = 1;
        for (auto& t : hudless) {
            t.Reset();
            if (FAILED(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &d, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, nullptr, IID_PPV_ARGS(&t))))
                return false;
        }
        hudless_w = w;
        hudless_h = h;
        return true;
    }

    // ------------------------------------------------------------------ swapchain
    bool create_swapchain() {
        if (FAILED(CreateDXGIFactory2(0, IID_PPV_ARGS(&factory)))) {
            fail("CreateDXGIFactory2 failed");
            return false;
        }
        ComPtr<IDXGIFactory5> f5;
        BOOL allow = FALSE;
        if (SUCCEEDED(factory.As(&f5)) && SUCCEEDED(f5->CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &allow, sizeof allow))) tearing = allow != FALSE;
        DXGI_SWAP_CHAIN_DESC1 d{};
        d.Width = width;
        d.Height = height;
        d.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
        d.SampleDesc.Count = 1;
        d.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        d.BufferCount = kBuffers;
        d.Scaling = DXGI_SCALING_STRETCH;
        d.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
        d.AlphaMode = DXGI_ALPHA_MODE_UNSPECIFIED;
        d.Flags = tearing ? DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING : 0;
        ComPtr<IDXGISwapChain1> sc1;
        if (sl_ok && fg != F2UP_FG_FSR) {
            // Streamline's proxy factory: Streamline has to see every Present (its per-frame
            // bookkeeping), and with DLSS-G its proxy swapchain inserts the generated frames.
            void* proxy = factory.Get();
            factory->AddRef();
            if (slUpgradeInterface(&proxy) == sl::Result::eOk) {
                IDXGIFactory2* pf = static_cast<IDXGIFactory2*>(proxy);
                if (SUCCEEDED(pf->CreateSwapChainForHwnd(queue.Get(), hwnd, &d, nullptr, nullptr, &sc1))) sc_proxied = true;
                else logf(2, "Streamline swapchain creation failed (DLSS Frame Generation off)");
                pf->Release();
            } else {
                factory->Release();
                logf(2, "slUpgradeInterface(factory) failed (DLSS Frame Generation off)");
            }
        } else if (fg == F2UP_FG_FSR && ffx_ok && fsr_fg_ok) {
            ffxCreateContextDescFrameGenerationSwapChainForHwndDX12 c{};
            c.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATIONSWAPCHAIN_FOR_HWND_DX12;
            IDXGISwapChain4* sc4 = nullptr;
            c.swapchain = &sc4;
            c.hwnd = hwnd;
            c.desc = &d;
            c.fullscreenDesc = nullptr;
            // With DLSS super resolution, Streamline's proxy factory underneath: Streamline has to
            // see the presents too.
            IDXGIFactory2* base = factory.Get();
            void* slproxy = nullptr;
            if (sl_ok) {
                factory->AddRef();
                slproxy = factory.Get();
                if (slUpgradeInterface(&slproxy) == sl::Result::eOk) base = static_cast<IDXGIFactory2*>(slproxy);
                else {
                    factory->Release();
                    slproxy = nullptr;
                }
            }
            c.dxgiFactory = base;
            c.gameQueue = queue.Get();
            ffxCreateContextDescFrameGenerationSwapChainVersionDX12 v{};
            v.header.type = FFX_API_CREATE_CONTEXT_DESC_TYPE_FRAMEGENERATIONSWAPCHAIN_VERSION_DX12;
            v.version = FFX_FRAMEGENERATION_SWAPCHAIN_DX12_VERSION;
            c.header.pNext = &v.header;
            if (fx.CreateContext(&sc_ctx, &c.header, nullptr) == FFX_API_RETURN_OK && sc4) {
                sc1.Attach(sc4);
                if (slproxy) logf(0, "FSR frame generation swapchain on Streamline's proxy factory");
            } else {
                sc_ctx = nullptr;
                logf(2, "FSR frame generation swapchain creation failed: frame generation off");
            }
            if (slproxy) static_cast<IUnknown*>(slproxy)->Release();
        }
        if (!sc1 && FAILED(factory->CreateSwapChainForHwnd(queue.Get(), hwnd, &d, nullptr, nullptr, &sc1))) {
            fail("CreateSwapChainForHwnd failed");
            return false;
        }
        factory->MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
        if (FAILED(sc1.As(&sc))) {
            fail("IDXGISwapChain3 unavailable");
            return false;
        }
        logf(0, "swapchain %ux%u (%s%s)", width, height, sc_proxied ? "Streamline proxy" : sc_ctx ? "FSR frame generation swapchain" : "DXGI",
             tearing ? ", tearing allowed" : "");
        return true;
    }

    // ------------------------------------------------------------------ frame
    bool evaluate(const f2up_frame_t* f) {
        ensure_token(f->frame);
        ID3D12GraphicsCommandList* l = begin_list();
        auto color = static_cast<ID3D12Resource*>(f->color);
        auto depth = static_cast<ID3D12Resource*>(f->depth);
        auto mv = static_cast<ID3D12Resource*>(f->motion);
        auto out = static_cast<ID3D12Resource*>(f->output);
        bool wrote = false;
        sr_active = F2UP_SR_NONE;
        const bool want_sl_frame = sl_ok && token && ((sr == F2UP_SR_DLSS && dlss_ok) || (sc_proxied && dlssg_ok));
        if (want_sl_frame) {
            sl::Constants c;
            copy4x4(f->view_to_clip, c.cameraViewToClip);
            copy4x4(f->clip_to_view, c.clipToCameraView);
            copy4x4(f->view_to_clip, c.clipToLensClip);
            copy4x4(f->clip_to_prev_clip, c.clipToPrevClip);
            copy4x4(f->prev_clip_to_clip, c.prevClipToClip);
            c.jitterOffset = sl::float2(f->jitter_x, f->jitter_y);
            c.mvecScale = sl::float2(1.0f / (float)f->render_w, 1.0f / (float)f->render_h);
            c.cameraPinholeOffset = sl::float2(0, 0);
            c.cameraPos = sl::float3(f->cam_pos[0], f->cam_pos[1], f->cam_pos[2]);
            c.cameraUp = sl::float3(f->cam_up[0], f->cam_up[1], f->cam_up[2]);
            c.cameraRight = sl::float3(f->cam_right[0], f->cam_right[1], f->cam_right[2]);
            c.cameraFwd = sl::float3(f->cam_fwd[0], f->cam_fwd[1], f->cam_fwd[2]);
            c.cameraNear = f->near_z;
            c.cameraFar = f->far_z;
            c.cameraFOV = f->fov_y;
            c.cameraAspectRatio = f->aspect;
            c.motionVectorsInvalidValue = sl::INVALID_FLOAT;
            c.depthInverted = sl::Boolean::eTrue;
            c.cameraMotionIncluded = sl::Boolean::eTrue;
            c.motionVectors3D = sl::Boolean::eFalse;
            c.reset = f->reset ? sl::Boolean::eTrue : sl::Boolean::eFalse;
            c.orthographicProjection = sl::Boolean::eFalse;
            c.motionVectorsDilated = sl::Boolean::eFalse;
            c.motionVectorsJittered = sl::Boolean::eFalse;
            if (slSetConstants(c, *token, viewport) != sl::Result::eOk) logf(1, "slSetConstants failed");
            sl::Extent rext{ 0, 0, f->render_w, f->render_h };
            sl::Extent oext{ 0, 0, f->output_w, f->output_h };
            auto mk = [](ID3D12Resource* r, uint32_t state, uint32_t w, uint32_t h, DXGI_FORMAT fmt) {
                sl::Resource s(sl::ResourceType::eTex2d, r, state);
                s.width = w;
                s.height = h;
                s.nativeFormat = fmt;
                s.flags = 0;
                return s;
            };
            sl::Resource rc = mk(color, kReadState, f->render_w, f->render_h, DXGI_FORMAT_R8G8B8A8_UNORM);
            sl::Resource rd = mk(depth, kReadState, f->render_w, f->render_h, DXGI_FORMAT_R32_FLOAT);
            sl::Resource rm = mk(mv, kReadState, f->render_w, f->render_h, DXGI_FORMAT_R16G16_FLOAT);
            sl::Resource ro = mk(out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, f->output_w, f->output_h, DXGI_FORMAT_R8G8B8A8_UNORM);
            std::vector<sl::ResourceTag> tags;
            tags.emplace_back(&rd, sl::kBufferTypeDepth, sl::ResourceLifecycle::eValidUntilPresent, &rext);
            tags.emplace_back(&rm, sl::kBufferTypeMotionVectors, sl::ResourceLifecycle::eValidUntilPresent, &rext);
            bool dlss = sr == F2UP_SR_DLSS && dlss_ok;
            if (dlss) {
                tags.emplace_back(&rc, sl::kBufferTypeScalingInputColor, sl::ResourceLifecycle::eValidUntilEvaluate, &rext);
                tags.emplace_back(&ro, sl::kBufferTypeScalingOutputColor, sl::ResourceLifecycle::eValidUntilEvaluate, &oext);
            }
            // The scene without the HUD: the upscaled image, or the render-size one when it is
            // the output size (no upscaler: it is what gets shown).
            // FSR writes the same output below, before the present that reads this.
            bool upscaled = dlss || ((sr == F2UP_SR_FSR3 || sr == F2UP_SR_FSR4) && ffx_ok);
            sl::Resource rh = upscaled ? ro : rc;
            if (sc_proxied && dlssg_ok && (upscaled || (f->render_w == f->output_w && f->render_h == f->output_h)))
                tags.emplace_back(&rh, sl::kBufferTypeHUDLessColor, sl::ResourceLifecycle::eValidUntilPresent, &oext);
            // The whole back buffer is the game image (no letterbox for frame generation to skip).
            if (sc_proxied && dlssg_ok) tags.emplace_back(nullptr, sl::kBufferTypeBackbuffer, sl::ResourceLifecycle::eValidUntilPresent, &oext);
            if (slSetTagForFrame(*token, viewport, tags.data(), (uint32_t)tags.size(), l) != sl::Result::eOk) logf(1, "slSetTagForFrame failed");
            if (dlss) {
                sl::DLSSOptions o = dlss_options(f->output_w, f->output_h);
                if (!dlss_set || memcmp(&o, &dlss_last, sizeof o) != 0) {
                    if (slDLSSSetOptions(viewport, o) != sl::Result::eOk) fail("slDLSSSetOptions failed");
                    dlss_last = o;
                    dlss_set = true;
                }
                const sl::BaseStructure* inputs[] = { &viewport };
                sl::Result r = slEvaluateFeature(sl::kFeatureDLSS, *token, inputs, 1, l);
                if (r == sl::Result::eOk) {
                    wrote = true;
                    sr_active = F2UP_SR_DLSS;
                } else {
                    static int n = 0;
                    if (n++ < 5) fail("DLSS evaluate failed (%d)", (int)r);
                }
            }
            apply_dlssg(f->render_w, f->render_h, f->output_w, f->output_h);
        }
        if ((sr == F2UP_SR_FSR3 || sr == F2UP_SR_FSR4) && ffx_ok && ensure_fsr_upscaler(sr, f->output_w, f->output_h)) {
            ffxDispatchDescUpscale d{};
            d.header.type = FFX_API_DISPATCH_DESC_TYPE_UPSCALE;
            d.commandList = l;
            d.color = ffx_res(color, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM);
            d.depth = ffx_res(depth, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R32_FLOAT);
            d.motionVectors = ffx_res(mv, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R16G16_FLOAT);
            d.output = ffx_res(out, FFX_API_RESOURCE_STATE_UNORDERED_ACCESS, FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM);
            d.jitterOffset = { f->jitter_x, f->jitter_y };
            d.motionVectorScale = { 1.0f, 1.0f };
            d.renderSize = { f->render_w, f->render_h };
            d.upscaleSize = { f->output_w, f->output_h };
            d.enableSharpening = opt.sharpness > 0.0f;
            d.sharpness = opt.sharpness;
            d.frameTimeDelta = f->frame_time_ms;
            d.preExposure = 1.0f;
            d.reset = f->reset != 0;
            d.cameraNear = f->near_z;
            d.cameraFar = f->far_z;
            d.cameraFovAngleVertical = f->fov_y;
            d.viewSpaceToMetersFactor = 0.01f;
            d.flags = FFX_UPSCALE_FLAG_NON_LINEAR_COLOR_SRGB;
            ffxReturnCode_t r = fx.Dispatch(&up_ctx, &d.header);
            if (r == FFX_API_RETURN_OK) {
                wrote = true;
                sr_active = (sr == F2UP_SR_FSR4 && !fsr4_id) ? F2UP_SR_FSR3 : sr;
            } else {
                static int n = 0;
                if (n++ < 5) fail("FSR dispatch failed (%u)", r);
            }
        }
        if (sc_ctx && fsr_fg_ok && ensure_fsr_fg(f->output_w, f->output_h) && ensure_hudless(f->output_w, f->output_h)) {
            // The HUD-less image is read when the frame is presented (and the generated one
            // after it): a copy per frame, double-buffered.
            ID3D12Resource* src = wrote ? out : color;
            D3D12_RESOURCE_STATES src_state = wrote ? D3D12_RESOURCE_STATE_UNORDERED_ACCESS : (D3D12_RESOURCE_STATES)kReadState;
            ID3D12Resource* hl = hudless[fg_frame_id & 1].Get();
            bool sized = wrote || (f->render_w == f->output_w && f->render_h == f->output_h);
            if (sized) {
                barrier(l, src, src_state, D3D12_RESOURCE_STATE_COPY_SOURCE);
                barrier(l, hl, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
                l->CopyResource(hl, src);
                barrier(l, hl, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
                barrier(l, src, D3D12_RESOURCE_STATE_COPY_SOURCE, src_state);
            }
            ffxConfigureDescFrameGeneration c{};
            c.header.type = FFX_API_CONFIGURE_DESC_TYPE_FRAMEGENERATION;
            c.swapChain = sc.Get();
            c.presentCallback = nullptr;
            c.frameGenerationCallback = [](ffxDispatchDescFrameGeneration* p, void* ctx) -> ffxReturnCode_t {
                return g_ffx_dispatch(reinterpret_cast<ffxContext*>(ctx), &p->header);
            };
            c.frameGenerationCallbackUserContext = &fg_ctx;
            c.frameGenerationEnabled = opt.fg_mode != 0;
            c.allowAsyncWorkloads = false;
            c.HUDLessColor = sized ? ffx_res(hl, FFX_API_RESOURCE_STATE_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R8G8B8A8_UNORM) : FfxApiResource{};
            c.flags = 0;
            c.onlyPresentGenerated = false;
            c.generationRect = { 0, 0, (int32_t)f->output_w, (int32_t)f->output_h };
            c.frameID = fg_frame_id;
            if (fx.Configure(&fg_ctx, &c.header) != FFX_API_RETURN_OK) logf(1, "FSR frame generation configure failed");
            ffxDispatchDescFrameGenerationPrepareV2 p{};
            p.header.type = FFX_API_DISPATCH_DESC_TYPE_FRAMEGENERATION_PREPARE_V2;
            p.frameID = fg_frame_id;
            p.flags = 0;
            p.commandList = l;
            p.renderSize = { f->render_w, f->render_h };
            p.jitterOffset = { f->jitter_x, f->jitter_y };
            p.motionVectorScale = { 1.0f, 1.0f };
            p.frameTimeDelta = f->frame_time_ms;
            p.reset = f->reset != 0;
            p.cameraNear = f->near_z;
            p.cameraFar = f->far_z;
            p.cameraFovAngleVertical = f->fov_y;
            p.viewSpaceToMetersFactor = 0.01f;
            p.depth = ffx_res(depth, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R32_FLOAT);
            p.motionVectors = ffx_res(mv, FFX_API_RESOURCE_STATE_PIXEL_COMPUTE_READ, FFX_API_SURFACE_FORMAT_R16G16_FLOAT);
            memcpy(p.cameraPosition, f->cam_pos, sizeof p.cameraPosition);
            memcpy(p.cameraUp, f->cam_up, sizeof p.cameraUp);
            memcpy(p.cameraRight, f->cam_right, sizeof p.cameraRight);
            memcpy(p.cameraForward, f->cam_fwd, sizeof p.cameraForward);
            if (fx.Dispatch(&fg_ctx, &p.header) != FFX_API_RETURN_OK) logf(1, "FSR frame generation prepare failed");
            ++fg_frame_id;
        }
        submit_list();
        return wrote;
    }

    bool present(ID3D12Resource* src) {
        if (!sc) return false;
        ComPtr<ID3D12Resource> bb;
        if (FAILED(sc->GetBuffer(sc->GetCurrentBackBufferIndex(), IID_PPV_ARGS(&bb)))) return false;
        ID3D12GraphicsCommandList* l = begin_list();
        barrier(l, bb.Get(), D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_DEST);
        l->CopyResource(bb.Get(), src);
        barrier(l, bb.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PRESENT);
        submit_list();
        bool markers = token && !token_presented;
        if (markers) {
            marker(sl::PCLMarker::eRenderSubmitEnd);
            marker(sl::PCLMarker::ePresentStart);
        }
        UINT sync = opt.vsync ? 1 : 0;
        UINT flags = (!opt.vsync && tearing) ? DXGI_PRESENT_ALLOW_TEARING : 0;
        HRESULT hr = sc->Present(sync, flags);
        if (markers) {
            marker(sl::PCLMarker::ePresentEnd);
            token_presented = true;
        }
        // On the presenting thread, after the present (Streamline asks for that).
        poll_dlssg();
        if (nr && ++presents == 120) nr_kick();
        if (FAILED(hr)) {
            fail("Present failed 0x%08lX", (unsigned long)hr);
            return false;
        }
        return true;
    }

    bool resize(uint32_t w, uint32_t h) {
        if (!sc || (w == width && h == height) || !w || !h) return true;
        wait_idle();
        destroy_fsr_fg();
        HRESULT hr = sc->ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, tearing ? DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING : 0);
        if (FAILED(hr)) {
            fail("ResizeBuffers failed 0x%08lX", (unsigned long)hr);
            return false;
        }
        width = w;
        height = h;
        dlssg_last_mode = -1;
        return true;
    }

    void nr_preload() {
        // RenoDX's DLSS add-on binds its Neural Rendering runtime lazily (docs/UPSCALING.md).
        std::wstring p = dir + L"\\nvngx_dlssnr.dll";
        if (GetFileAttributesW(p.c_str()) != INVALID_FILE_ATTRIBUTES && LoadLibraryW(p.c_str())) logf(0, "loaded nvngx_dlssnr.dll for DLSS 5");
    }

    void nr_kick() {
        // Once the game runs: a throwaway WARP device makes ReShade raise init_device again, and
        // RenoDX attaches its Neural Rendering runtime with the game's device and DLSS live.
        logf(0, "DLSS 5: WARP device to let RenoDX attach Neural Rendering");
        ComPtr<IDXGIFactory4> f4;
        ComPtr<IDXGIAdapter> warp_adapter;
        if (SUCCEEDED(CreateDXGIFactory2(0, IID_PPV_ARGS(&f4))) && SUCCEEDED(f4->EnumWarpAdapter(IID_PPV_ARGS(&warp_adapter))))
            D3D12CreateDevice(warp_adapter.Get(), D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&warp));
    }

    ~f2up() {
        if (queue && fence) wait_idle();
        if (ffx_ok) {
            destroy_fsr_fg();
            destroy_fsr_upscaler();
        }
        if (sl_ok && slFreeResources) {
            if (dlss_ok) slFreeResources(sl::kFeatureDLSS, viewport);
            if (dlssg_ok && sc_proxied) slFreeResources(sl::kFeatureDLSS_G, viewport);
        }
        sc.Reset();
        if (sc_ctx) fx.DestroyContext(&sc_ctx, nullptr);
        factory.Reset();
        if (sl_ok && slShutdown) slShutdown();
        if (fence_event) CloseHandle(fence_event);
    }
};

extern "C" {

f2up* f2up_create(const f2up_init_t* in, char* err, uint32_t errlen) {
    g_log = in->log;
    auto* u = new f2up();
    u->dev = static_cast<ID3D12Device*>(in->device);
    u->queue = static_cast<ID3D12CommandQueue*>(in->queue);
    u->hwnd = static_cast<HWND>(in->hwnd);
    u->dir = in->runtime_dir ? in->runtime_dir : L".";
    u->logdir = in->log_dir ? in->log_dir : L"";
    u->sr = in->sr;
    u->fg = in->fg;
    u->width = in->width ? in->width : 1;
    u->height = in->height ? in->height : 1;
    u->opt.vsync = 1;
    auto bail = [&](const char* why) -> f2up* {
        snprintf(err, errlen, "%s", why[0] ? why : u->error);
        delete u;
        return nullptr;
    };
    if (!u->init_lists()) return bail("command list creation failed");
    if (in->sr == F2UP_SR_DLSS || in->fg == F2UP_FG_DLSS) u->load_sl(in->sr == F2UP_SR_DLSS, in->fg == F2UP_FG_DLSS);
    if (in->sr == F2UP_SR_FSR3 || in->sr == F2UP_SR_FSR4 || in->fg == F2UP_FG_FSR) u->load_ffx();
    if (!u->create_swapchain()) return bail("");
    u->nr = in->nr_helpers != 0;
    if (u->nr) u->nr_preload();
    return u;
}

void f2up_destroy(f2up* u) { delete u; }

void f2up_set_backend(f2up* u, uint32_t sr) {
    if (u->sr == sr) return;
    u->sr = sr;
    if (sr == F2UP_SR_DLSS) u->load_sl(true, false);
    if (sr == F2UP_SR_FSR3 || sr == F2UP_SR_FSR4) u->load_ffx();
    u->dlss_set = false;
}

void f2up_set_options(f2up* u, const f2up_options_t* o) {
    bool reflex = o->reflex != u->opt.reflex || o->fg_mode != u->opt.fg_mode;
    u->opt = *o;
    if (reflex) u->apply_reflex();
    u->dlssg_last_mode = -1;
}

void f2up_render_size(f2up* u, uint32_t ow, uint32_t oh, uint32_t* rw, uint32_t* rh) {
    *rw = *rh = 0;
    if (u->sr == F2UP_SR_DLSS && u->dlss_ok && u->slDLSSGetOptimalSettings) {
        sl::DLSSOptions o = u->dlss_options(ow, oh);
        sl::DLSSOptimalSettings s;
        if (u->slDLSSGetOptimalSettings(o, s) == sl::Result::eOk && s.optimalRenderWidth && s.optimalRenderHeight) {
            *rw = s.optimalRenderWidth;
            *rh = s.optimalRenderHeight;
        }
    }
}

void f2up_begin_frame(f2up* u, uint64_t frame) {
    if (!u->sl_ok) return;
    u->ensure_token(frame);
    if (!u->token) return;
    if (u->reflex_ok && u->slReflexSleep) u->slReflexSleep(*u->token);
    u->marker(sl::PCLMarker::eSimulationStart);
    u->marker(sl::PCLMarker::eSimulationEnd);
    u->marker(sl::PCLMarker::eRenderSubmitStart);
}

int f2up_evaluate(f2up* u, const f2up_frame_t* f) { return u->evaluate(f) ? 1 : 0; }

int f2up_present(f2up* u, void* src) { return u->present(static_cast<ID3D12Resource*>(src)) ? 1 : 0; }

int f2up_resize(f2up* u, uint32_t w, uint32_t h) { return u->resize(w, h) ? 1 : 0; }

void f2up_status(f2up* u, f2up_status_t* s) {
    memset(s, 0, sizeof *s);
    s->sl_loaded = u->sl_ok;
    s->dlss_supported = u->dlss_ok;
    s->dlssg_supported = u->dlssg_ok && u->sc_proxied;
    s->dlssg_max_frames = u->dlssg_max;
    s->dlssg_dynamic = u->dlssg_dynamic;
    s->reflex_supported = u->reflex_ok;
    s->ffx_loaded = u->ffx_ok;
    s->fsr3_available = u->fsr3_id != 0;
    s->fsr4_available = u->fsr4_id != 0;
    s->fsr_fg_available = u->fsr_fg_ok && u->sc_ctx != nullptr;
    s->sr_active = u->sr_active;
    s->fg_backend = (u->sc_proxied && u->dlssg_ok) ? F2UP_FG_DLSS : u->sc_ctx ? F2UP_FG_FSR : F2UP_FG_NONE;
    s->fg_active = s->fg_backend != F2UP_FG_NONE && u->opt.fg_mode != 0;
    s->presented = u->dlssg_presented;
    std::string sr = u->sr_active == F2UP_SR_DLSS ? "DLSS " + u->dlss_version : (u->sr_active ? u->up_name : std::string());
    std::string fg = (u->sc_proxied && u->dlssg_ok) ? "DLSS Frame Generation " + u->dlssg_version : u->sc_ctx ? u->fg_name : std::string();
    snprintf(s->sr_name, sizeof s->sr_name, "%s", sr.c_str());
    snprintf(s->fg_name, sizeof s->fg_name, "%s", fg.c_str());
    snprintf(s->error, sizeof s->error, "%s", u->error);
}

} // extern "C"
