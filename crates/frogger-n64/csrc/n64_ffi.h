/* Narrow C <-> Rust interface of the N64 host (mirrored in src/ffi.rs).
 *
 * ABI (see tools/n64/README.md "ABI"): C is libdragon's o64, Rust is n32. Both pass the
 * first four integer/pointer arguments in $a0-$a3 (sign-extended 32-bit values in 64-bit
 * registers) and return in $v0, so every function here takes AT MOST FOUR arguments, each a
 * 32-bit integer or a pointer (64-bit integers are fine too: one register in both ABIs),
 * and NO floating-point arguments (floats travel inside structs behind pointers).
 *
 *  - Rust -> C: each n64_* symbol is an assembly stub (abi.S) that gives the C body
 *    (n64_*_impl) the 32-byte argument home area o64 callees may write, which an n32
 *    caller does not reserve.
 *  - C -> Rust: only through f2n64_call (abi.S), which saves the o64 callee-saved FPRs
 *    that n32 does not preserve ($f21..$f31 odd), aligns $sp to 16 and runs Rust with the
 *    FPU trap enables cleared (Rust assumes IEEE non-trapping arithmetic).
 */
#ifndef F2_N64_FFI_H
#define F2_N64_FFI_H
#include <stdint.h>

/* One vertex of a screen-space triangle: TRIFMT_ZBUF_SHADE_TEX layout.
 * x, y pixels; z 0..1; r, g, b, a 0..1; s, t texels; inv_w (1 = affine). */
typedef struct { float x, y, z, r, g, b, a, s, t, inv_w; } n64_vertex_t;

/* Render state of a triangle batch. */
enum {
    N64_ZTEST = 1 << 0,     /* depth compare (LEQUAL-ish: RDP "less or equal" with z-source per pixel) */
    N64_ZWRITE = 1 << 1,
    N64_BILINEAR = 1 << 2,
    N64_WRAP = 1 << 3,      /* texture repeat (else clamp) */
    N64_PERSP = 1 << 4,     /* perspective-correct texturing (uses inv_w) */
    N64_UVNORM = 1 << 5,    /* triangles: s, t are normalised 0..1 (scaled to the TMEM-sized surface) */
    N64_FOG = 1 << 6,       /* r3d: blender fog (shade alpha = fog factor) */
    N64_TRUE_ADDITIVE = 1 << 7, /* r3d: real (wrapping) RDP additive instead of the approximation */
};
enum { N64_BLEND_NONE = 0, N64_BLEND_SEMI = 1, N64_BLEND_ADD = 2, N64_BLEND_SUB = 3 };
typedef struct {
    uint32_t flags;
    uint32_t blend;
    uint32_t tex;       /* handle from n64_tex_create, 0 = untextured (shade only) */
    uint32_t alpha_ref; /* alpha compare threshold 0..255; 0 = off */
} n64_state_t;

/* Texture description for n64_tex_create. fmt: 0 = RGBA16 (5551), 1 = RGBA32; | N64_TEX_TILE3D
 * (on the full image): triangles sample the full image in TMEM tiles instead of the small one. */
#define N64_TEX_TILE3D 0x100
typedef struct {
    uint32_t width, height, fmt;
    const void *pixels; /* tightly packed rows */
} n64_tex_desc_t;

/* A screen-aligned textured (or flat) rectangle: x/y pixels, s/t normalised 0..1 (s1 < s0
 * or t1 < t0 flips), rgba = 0xRRGGBBAA modulating the texture, z = depth 0..1 (primitive
 * depth, used when the state has N64_ZTEST: scene sprites). Drawn with texture rectangles
 * (rdpq_tex_blit splits textures larger than TMEM). */
typedef struct { float x0, y0, x1, y1, s0, t0, s1, t1; uint32_t rgba; float z; } n64_rect_t;

/* An NTX1 image (1C pack) as a texture: texels/tlut stay in the caller's buffer. fmt: NTX
 * format (0 RGBA16, 1 RGBA32, 2 CI4, 3 CI8, 4 IA4, 5 IA8, 6 I4, 7 I8, 8 IA16). */
enum { N64_NTX_SINGLE = 1 << 0, N64_NTX_PER_TILE = 1 << 1, N64_NTX_TILED = 1 << 2 /* triangles always sample the full image in TMEM tiles */ };
typedef struct {
    uint32_t fmt, width, height, stride;
    const void *texels;
    const void *tlut;
    uint32_t tlut_count, flags, tile_w, tile_h;
} n64_ntx_desc_t;

/* A sound-effect voice: linear left/right volume, playback frequency in Hz (0 = keep). */
typedef struct { float lvol, rvol, freq; } n64_voice_t;

typedef struct { uint32_t total, used; } n64_heap_t;
/* ---- r3d: RSP 3D renderer (r3d.c; Rust side src/r3d.rs) ---- */

/* An F2NM texture (crates/f2-n64fmt) to register. texels/tlut stay owned by the caller and
 * must stay alive (8-byte aligned RDRAM). fmt: f2-n64fmt TexFormat. */
typedef struct {
    uint32_t fmt, width, height, tlut_count;
    const void *texels;
    const void *tlut;       /* RGBA16 palette (CI formats) or NULL */
} n64_r3d_tex_desc_t;

/* One texture group of an F2NM mesh: its vertex array and command words (big-endian =
 * native). Rigid groups are recorded into an rspq block at creation. */
typedef struct {
    const void *verts;      /* the mesh's whole vertex array (16 bytes per vertex) */
    const uint32_t *cmds;   /* this group's commands */
    uint32_t n_verts, n_cmds;
    uint32_t skinned;
} n64_r3d_group_desc_t;

/* F2_DRAW uniform (96 bytes): the model matrix as floats (rows x, y, z, translation; lanes
 * x y z; the shader converts it to s15.16, see rsp_f2mdx.S F2_Convert), the conversion flag
 * (0 from the CPU), colour multiplier/addend, fog. */
typedef struct {
    float mtx[12];
    uint32_t cvt[2];
    int16_t cmul[4];
    int16_t cadd[4];
    int16_t fogc[4];        /* 0, constant unfogged fraction, 0, 0 */
    uint16_t fogm[4];       /* VCC masks: keep colour alpha (else constant), per-vertex fog */
    uint32_t pad[2];
} __attribute__((aligned(8))) n64_r3d_draw_uniform_t;

/* F2_FRAME uniform (128 bytes) + host fields. clip_* / modge_* are filled by C. */
typedef struct n64_r3d_frame_s {
    int16_t vo[4];          /* x, y screen offset (s13.2), Zoff, 0 */
    int16_t vs[4];          /* S, S (-8 * FOV * scale), K integer, 0 */
    int16_t vlo[4];         /* x, y screen-reject low bound (s13.2), K fraction, 0 */
    int16_t vhi[4];         /* x, y screen-reject high bound (s13.2), 0, 0 */
    int16_t wone[4];        /* 1, 0, 1, 0 */
    int16_t fogf[4];        /* 0, 0, -32767 / (end - start), end - 10 */
    int16_t vp_i[16];       /* view x projection (rows; lanes X, Y, Z, W), s15.16 integer halves */
    uint16_t vp_f[16];      /* fraction halves */
    uint32_t clip_text, clip_size;
    uint32_t modge_text, modge_size;
    uint32_t guard;         /* guard band in multiples of FOV*scale pixels (0 = 4) */
    uint32_t fog_rgba;      /* fog colour, 0 = leave */
} __attribute__((aligned(8))) n64_r3d_frame_t;

/* One draw: mesh groups with resolved texture handles. */
typedef struct {
    const n64_r3d_draw_uniform_t *u;      /* RDRAM, valid until the RSP ran the frame */
    const uint16_t *groups;               /* group handles */
    const uint16_t *texs;                 /* texture handle per group */
    uint32_t n_groups;
    const n64_r3d_draw_uniform_t *bones;  /* skinned: uniform per bone (RDRAM, loaded by DMA), else NULL */
    uint32_t n_bones;
    const struct n64_r3d_frame_s *frame;  /* frame uniform for this draw (sky), NULL = the frame's */
} n64_r3d_draw_t;

/* Render state of a frame list. */
typedef struct {
    uint32_t blend;         /* N64_BLEND_* */
    uint32_t flags;         /* N64_ZTEST | N64_ZWRITE | N64_BILINEAR | N64_WRAP | N64_FOG | ... */
    uint32_t alpha_ref;     /* pixels with alpha < alpha_ref are discarded (0 = off) */
    uint32_t cull;          /* 0 = none, 1 = cull clockwise front faces, 3 = counter-clockwise */
} n64_r3d_list_t;

typedef struct {
    uint32_t draws, groups, tex_loads;
    uint32_t ticks_state, ticks_draws; /* C0 ticks: list render states, draw loop */
} n64_r3d_stats_t;

/* Rust -> C */
void n64_log(const char *msg, uint32_t len);
void n64_panic(const char *msg, uint32_t len) __attribute__((noreturn));
void *n64_alloc(uint32_t size, uint32_t align);
void n64_free(void *ptr);
uint32_t n64_ticks(void);                          /* C0 COUNT: 46.875 MHz (CPU clock / 2) */
uint32_t n64_pad(uint32_t port);                   /* buttons (joypad_buttons_t.raw) | stick x << 16 | stick y << 24 */
void *n64_file_read(const char *path, uint32_t *len); /* asset_load: malloc'd, freed with n64_free; NULL if missing */
uint32_t n64_tex_create(const n64_tex_desc_t *desc);
/* full: the image (2D blits); small: a TMEM-sized version for triangles, NULL = full fits. */
uint32_t n64_tex_create2(const n64_tex_desc_t *full, const n64_tex_desc_t *small);
void n64_tex_free(uint32_t handle);
uint32_t n64_tex_update(uint32_t handle, const n64_tex_desc_t *desc); /* in-place texel update; 0 = not possible */
/* In-place update written by the caller (desc->pixels unused): the texture's texel buffer
 * (rows of width * bpp bytes, tightly packed; uncached memory) or NULL if it cannot be
 * updated in place; then n64_tex_update_end. */
void *n64_tex_update_begin(uint32_t handle, const n64_tex_desc_t *desc);
void n64_tex_update_end(uint32_t handle);
uint32_t n64_tex_create_ntx(const n64_ntx_desc_t *desc);         /* texture over an NTX1 image (no copy) */
void n64_begin_frame(uint32_t clear_rgba);         /* attach next framebuffer + z-buffer, clear both */
void n64_draw_tris(const n64_vertex_t *verts, uint32_t ntris, const n64_state_t *state);
void n64_draw_rects(const n64_rect_t *rects, uint32_t n, const n64_state_t *state);
/* Scene triangles straight from f2-engine's frame lists (phase 3E: no Rust-side vertex
 * conversion): `ntris` triangles of three TlVertex (12 floats: sx sy sz rhw r g b a u v fog
 * aref) each, `stride` bytes apart from `tris`; z = clamp(sz * zk); the colour split of the
 * state's blend (additive / subtractive) as render_cpu.rs applies it. */
typedef struct { const void *tris; uint32_t stride, ntris; float zk; } n64_tl_batch_t;
void n64_draw_tl_tris(const n64_tl_batch_t *batch, const n64_state_t *state);
void n64_cache_writeback(const void *p, uint32_t len);
void n64_present(void);                            /* detach + show */
void n64_set_fps_limit(uint32_t fps);                /* display_set_fps_limit (0 = none) */
void n64_rdp_wait(void);                           /* block until the RSP/RDP queue is idle (probes) */
void n64_beep(uint32_t freq_hz, uint32_t ms);
uint32_t n64_audio_stats(void);                    /* samples generated by the beep waveform so far */
uint32_t n64_memory_size(void);
void n64_heap_stats(n64_heap_t *out);
/* Allocation phase: 1 = a level is loading (everything from the level heap, a new load epoch),
 * 0 = play (main heap). Returns the previous phase. */
uint32_t n64_mem_phase(uint32_t phase);
/* `mem [tag]` log line: both heaps; `check`: list level-heap blocks of earlier loads. */
void n64_mem_report(const char *tag, uint32_t len, uint32_t check);
void n64_mem_reset_peaks(void);
/* Sound effects: wav64 files streamed from DFS. handle 0 = missing. */
uint32_t n64_sfx_open(const char *path, uint32_t looped);
void n64_sfx_close(uint32_t handle);
uint32_t n64_sfx_channels(void);
uint32_t n64_sfx_freq(uint32_t handle);            /* native rate of an opened sample, Hz */
uint32_t n64_rcp_load(void);                       /* RSP busy permille | RDP busy permille << 16 since the last call */
void n64_flip_stats(uint32_t *hist);                /* flips by vblanks the last image was shown (6 entries: [1..4], [5] = 5+); resets */
uint32_t n64_fmv_play(const char *path);           /* blocking video playback; 0 = missing */
void n64_audio_poll_due(void); /* n64_audio_poll if 40 ms have passed since the last */
uint32_t n64_audio_gap_take(void); /* longest gap between audio polls (C0 ticks) since the last call */
uint32_t n64_save_read(void *dst, uint32_t off, uint32_t len);   /* SRAM[off..] -> dst; returns len or 0 */
uint32_t n64_save_write(const void *src, uint32_t off, uint32_t len); /* src -> SRAM[off..]; returns len or 0 */                   /* first sfx channel << 8 | count */
void n64_sfx_play(uint32_t handle, uint32_t ch, const n64_voice_t *v);
void n64_ch_set(uint32_t ch, const n64_voice_t *v);
void n64_ch_stop(uint32_t ch);
uint32_t n64_ch_playing(uint32_t ch);
uint32_t n64_music_play(const char *path, uint32_t looped); /* NULL path = stop; 0 = missing */
void n64_music_vol(uint32_t vol_permille);
void n64_audio_poll(void);
void n64_prof(uint32_t cmd);                       /* sampling profiler: 0 = start, 1 = dump + reset, 2 = pause, 3 = resume */                         /* feed the mixer if the AI wants data */
/* C-side probes (see main.c): 0/4 = rdpq_triangle loops, 1/2/3 = rspq block record/end/run. */
uint32_t n64_probe_c(uint32_t kind, uint32_t n);
void n64_r3d_init(void);
uint32_t n64_r3d_tex_create(const n64_r3d_tex_desc_t *desc);     /* -> texture handle (0 = failed) */
uint32_t n64_r3d_group_create(const n64_r3d_group_desc_t *desc); /* -> group handle (0 = failed) */
void n64_r3d_begin(const n64_r3d_frame_t *frame);                /* after n64_begin_frame */
void n64_r3d_draw_list(const n64_r3d_list_t *state, const n64_r3d_draw_t *draws, uint32_t n);
void n64_r3d_stats(n64_r3d_stats_t *out);
uint32_t n64_r3d_tex_alias(uint32_t tex);                        /* r3d texture over a main.c texture's TMEM surface */
void n64_r3d_reset(uint32_t tex_first, uint32_t group_first);    /* free r3d textures/groups from these handles on */
uint32_t n64_r3d_counts(void);                                   /* next texture handle << 16 | next group handle */
void n64_r3d_modge(const void *desc, uint32_t size);
void n64_r3d_cvt_check(const float *vals, uint32_t n, int16_t *out_i, uint16_t *out_f); /* testing */            /* RSP water animation (F2_Modge), after n64_r3d_begin */

/* C -> Rust */
void f2n64_init(void);
void f2n64_frame(void);
void f2n64_call(void (*fn)(void));
#endif
