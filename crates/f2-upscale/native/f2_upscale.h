/* f2_upscale: DLSS (NVIDIA Streamline), FSR 3.1 / FSR 4 (AMD FidelityFX API) super resolution
 * and frame generation on a Direct3D 12 device and queue someone else created (wgpu's), with
 * its own DXGI swapchain (frame generation has to own the present).
 *
 * The C ABI the Rust side (`src/lib.rs`) mirrors. Every call is made from one thread. Resources
 * are ID3D12Resource pointers in the states documented per call; they are left in them.
 */
#pragma once
#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*f2up_log_fn)(int level, const char* msg); /* 0 info, 1 warning, 2 error */

/* Super resolution back end. */
enum { F2UP_SR_NONE = 0, F2UP_SR_DLSS = 1, F2UP_SR_FSR3 = 2, F2UP_SR_FSR4 = 3 };
/* Frame generation back end (fixed for the swapchain's lifetime). */
enum { F2UP_FG_NONE = 0, F2UP_FG_DLSS = 1, F2UP_FG_FSR = 2 };

typedef struct {
    void* device;              /* ID3D12Device* */
    void* queue;               /* ID3D12CommandQueue* (direct): everything is submitted here */
    void* hwnd;                /* window to present to */
    const wchar_t* runtime_dir; /* sl.*.dll + nvngx_*.dll, amd_fidelityfx_*_dx12.dll */
    const wchar_t* log_dir;    /* Streamline's log (sl.log) */
    uint32_t sr;               /* F2UP_SR_* (may change later with f2up_set_backend) */
    uint32_t fg;               /* F2UP_FG_* */
    uint32_t width, height;    /* swapchain */
    f2up_log_fn log;
    /* DLSS 5 (RenoDX's NGX-hooking ReShade add-on): load nvngx_dlssnr.dll and create one WARP
     * device after start-up so the add-on attaches its runtime (see docs/UPSCALING.md). */
    uint32_t nr_helpers;
} f2up_init_t;

typedef struct {
    uint32_t sr_mode;      /* 0 native AA (DLAA / FSR Native AA), 1 quality, 2 balanced, 3 performance, 4 ultra performance */
    uint32_t dlss_preset;  /* 0 default, else sl::DLSSPreset (10 J, 11 K, 12 L, 13 M) */
    float sharpness;       /* FSR RCAS 0..1, 0 = off (DLSS has none) */
    uint32_t fg_mode;      /* 0 off, 1 2x, 2 3x, 3 4x (DLSS only), 4 dynamic to fg_target_fps (DLSS only) */
    float fg_target_fps;   /* dynamic frame generation target, 0 = display refresh */
    uint32_t reflex;       /* 0 off, 1 on, 2 on + boost (Streamline) */
    uint32_t vsync;
} f2up_options_t;

/* One frame's inputs (Renderer::render_upscaled). color/depth/motion: render size, in
 * PIXEL_SHADER_RESOURCE | NON_PIXEL_SHADER_RESOURCE; output: output size, UNORDERED_ACCESS. */
typedef struct {
    void* color;   /* R8G8B8A8_UNORM, gamma space */
    void* depth;   /* R32_FLOAT, reversed infinite (near / z) */
    void* motion;  /* R16G16_FLOAT, render pixels, previous minus current */
    void* output;  /* R8G8B8A8_UNORM */
    uint32_t render_w, render_h, output_w, output_h;
    float jitter_x, jitter_y; /* render pixels */
    uint32_t reset;
    float frame_time_ms;
    /* Row-major, row-vector (v' = v * M) matrices, as Streamline takes them. */
    float view_to_clip[16], clip_to_view[16], clip_to_prev_clip[16], prev_clip_to_clip[16];
    float cam_pos[3], cam_up[3], cam_right[3], cam_fwd[3];
    float near_z, far_z, fov_y, aspect;
    uint64_t frame;
    /* Optional (NULL = none): R8_UNORM, render size, 1 where the history must not be trusted
     * (on/off flickering textures): DLSS bias-current-colour hint, FSR reactive mask. */
    void* reactive;
} f2up_frame_t;

typedef struct {
    uint32_t sl_loaded, dlss_supported, dlssg_supported, dlssg_max_frames, dlssg_dynamic, reflex_supported;
    uint32_t ffx_loaded, fsr3_available, fsr4_available, fsr_fg_available;
    uint32_t sr_active;   /* F2UP_SR_* that ran last frame */
    uint32_t fg_active;   /* frame generation on (generating) */
    uint32_t fg_backend;  /* F2UP_FG_* of the swapchain */
    uint32_t presented;   /* DLSS-G: frames presented per rendered frame last second */
    char sr_name[64];     /* e.g. "DLSS 310.8.0", "FSR 3.1.5" */
    char fg_name[64];
    char error[256];      /* last error, empty if none */
} f2up_status_t;

typedef struct f2up f2up;

/* NULL on failure (err gets the reason). The swapchain is created here. */
f2up* f2up_create(const f2up_init_t* init, char* err, uint32_t errlen);
void f2up_destroy(f2up* u);
/* Switches the super resolution back end (loads its library on first use). */
void f2up_set_backend(f2up* u, uint32_t sr);
void f2up_set_options(f2up* u, const f2up_options_t* o);
/* The render size the active back end wants for an output size and the current mode (DLSS:
 * its optimal settings); 0 = the caller's default ratios. */
void f2up_render_size(f2up* u, uint32_t out_w, uint32_t out_h, uint32_t* render_w, uint32_t* render_h);
/* Start of a frame: frame token, Reflex sleep, latency markers. */
void f2up_begin_frame(f2up* u, uint64_t frame);
/* Upscales into f->output (1 = written) and hands frame generation this frame's inputs. */
int f2up_evaluate(f2up* u, const f2up_frame_t* f);
/* Copies src (R8G8B8A8_UNORM, swapchain size, COPY_SOURCE) to the back buffer and presents. */
int f2up_present(f2up* u, void* src);
/* Waits for the GPU and resizes the swapchain. */
int f2up_resize(f2up* u, uint32_t w, uint32_t h);
void f2up_status(f2up* u, f2up_status_t* s);

#ifdef __cplusplus
}
#endif
