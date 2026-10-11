/* libdragon host of the Frogger 2 N64 port: boot, display, RDP, audio, joypad, DFS.
 * Rust (libfrogger_n64.a) owns the frame: main() calls f2n64_init() once and f2n64_frame()
 * per frame through the f2n64_call bridge; Rust calls back the n64_* functions below
 * (via the abi.S stubs). Interface and ABI rules: n64_ffi.h. */
#include <libdragon.h>
#include <malloc.h>
#include <string.h>
#include "n64_ffi.h"

#define MAX_TEXTURES 256

static surface_t zbuf;
static surface_t textures[MAX_TEXTURES];
static uint32_t num_textures = 1; /* handle 0 = none */
static bool attached;

/* Last applied RDP mode, to skip redundant mode changes between batches. */
static n64_state_t cur_state;
static bool cur_valid;
static uint32_t cur_tex_loaded;

/* ---- audio: a generated square-wave beep through the mixer ---- */
#define BEEP_CH 0
static waveform_t beep_wave;
static volatile uint32_t beep_freq = 880, beep_samples_out;

static void beep_read(void *ctx, samplebuffer_t *sbuf, int wpos, int wlen, bool seeking) {
    int16_t *out = samplebuffer_append(sbuf, wlen);
    uint32_t period = 44100 / beep_freq;
    for (int i = 0; i < wlen; i++) {
        uint32_t p = (uint32_t)(wpos + i) % period;
        out[i] = p < period / 2 ? 6000 : -6000;
    }
    beep_samples_out += wlen;
}

void n64_beep_impl(uint32_t freq_hz, uint32_t ms) {
    beep_freq = freq_hz ? freq_hz : 880;
    beep_wave = (waveform_t){
        .name = "beep", .bits = 16, .channels = 1, .frequency = 44100,
        .len = (int)(44100 * ms / 1000), .loop_len = 0, .read = beep_read, .ctx = NULL,
    };
    mixer_ch_play(BEEP_CH, &beep_wave);
}

uint32_t n64_audio_stats_impl(void) { return beep_samples_out; }

/* ---- misc ---- */
void n64_log_impl(const char *msg, uint32_t len) { debugf("%.*s", (int)len, msg); }

void n64_panic_impl(const char *msg, uint32_t len) {
    debugf("RUST PANIC: %.*s\n", (int)len, msg);
    assertf(0, "Rust panic: %.*s", (int)len, msg);
    abort();
}

void *n64_alloc_impl(uint32_t size, uint32_t align) {
    return memalign(align < 8 ? 8 : align, size ? size : 1);
}

void n64_free_impl(void *ptr) { free(ptr); }

uint32_t n64_ticks_impl(void) { return TICKS_READ(); }

uint32_t n64_memory_size_impl(void) { return get_memory_size(); }

uint32_t n64_pad_impl(uint32_t port) {
    if (port > 3) return 0;
    joypad_buttons_t b = joypad_get_buttons(port);
    joypad_inputs_t in = joypad_get_inputs(port);
    return (uint32_t)b.raw | ((uint32_t)(uint8_t)in.stick_x << 16) | ((uint32_t)(uint8_t)in.stick_y << 24);
}

void *n64_file_read_impl(const char *path, uint32_t *len) {
    int sz = 0;
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fclose(f);
    void *p = asset_load(path, &sz);
    if (len) *len = (uint32_t)sz;
    return p;
}

uint32_t n64_tex_create_impl(const n64_tex_desc_t *d) {
    if (num_textures >= MAX_TEXTURES) return 0;
    tex_format_t fmt = d->fmt == 1 ? FMT_RGBA32 : FMT_RGBA16;
    surface_t s = surface_alloc(fmt, d->width, d->height);
    uint32_t row = TEX_FORMAT_PIX2BYTES(fmt, d->width);
    for (uint32_t y = 0; y < d->height; y++)
        memcpy((uint8_t *)s.buffer + y * s.stride, (const uint8_t *)d->pixels + y * row, row);
    textures[num_textures] = s;
    return num_textures++;
}

/* ---- frame ---- */
void n64_begin_frame_impl(uint32_t rgba) {
    surface_t *fb = display_get();
    rdpq_attach(fb, &zbuf);
    rdpq_set_mode_fill(color_from_packed32(rgba));
    rdpq_fill_rectangle(0, 0, display_get_width(), display_get_height());
    rdpq_clear_z(ZBUF_MAX);
    attached = true;
    cur_valid = false;
    cur_tex_loaded = 0;
}

static void apply_state(const n64_state_t *st) {
    if (cur_valid && memcmp(st, &cur_state, sizeof *st) == 0) return;
    if (!cur_valid || st->flags != cur_state.flags || st->blend != cur_state.blend ||
        st->alpha_ref != cur_state.alpha_ref || (st->tex == 0) != (cur_state.tex == 0)) {
        rdpq_set_mode_standard();
        rdpq_mode_combiner(st->tex ? RDPQ_COMBINER_TEX_SHADE : RDPQ_COMBINER_SHADE);
        rdpq_mode_zbuf(st->flags & N64_ZTEST, st->flags & N64_ZWRITE);
        rdpq_mode_filter(st->flags & N64_BILINEAR ? FILTER_BILINEAR : FILTER_POINT);
        rdpq_mode_persp(st->flags & N64_PERSP);
        rdpq_mode_alphacompare((int)st->alpha_ref);
        switch (st->blend) {
        case N64_BLEND_SEMI: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        case N64_BLEND_ADD: rdpq_mode_blender(RDPQ_BLENDER_ADDITIVE); break;
        /* TODO(phase 2): D3D ZERO/INVSRCCOLOR has no exact RDP blender equivalent (the
         * blender only has alpha factors); approximated as multiply for now. */
        case N64_BLEND_SUB: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        default: rdpq_mode_blender(0); break;
        }
    }
    if (st->tex && st->tex < num_textures && (st->tex != cur_tex_loaded || ((st->flags ^ cur_state.flags) & N64_WRAP))) {
        rdpq_texparms_t parms = {0};
        if (st->flags & N64_WRAP) parms.s.repeats = parms.t.repeats = REPEAT_INFINITE;
        rdpq_tex_upload(TILE0, &textures[st->tex], &parms);
        cur_tex_loaded = st->tex;
    }
    cur_state = *st;
    cur_valid = true;
}

void n64_draw_tris_impl(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    if (!attached || !ntris) return;
    apply_state(st);
    /* Our vertices always have the 10-float ZBUF_SHADE_TEX layout; describe the subset used. */
    static rdpq_trifmt_t fmts[4];
    int i = (st->tex ? 1 : 0) | ((st->flags & (N64_ZTEST | N64_ZWRITE)) ? 2 : 0);
    rdpq_trifmt_t *f = &fmts[i];
    *f = (rdpq_trifmt_t){
        .pos_offset = 0, .z_offset = (i & 2) ? 2 : -1, .shade_offset = 3,
        .tex_offset = (i & 1) ? 7 : -1, .tex_tile = TILE0,
    };
    for (uint32_t t = 0; t < ntris; t++, v += 3)
        rdpq_triangle(f, (const float *)&v[0], (const float *)&v[1], (const float *)&v[2]);
}

void n64_present_impl(void) {
    if (!attached) return;
    rdpq_detach_show();
    attached = false;
}

void n64_rdp_wait_impl(void) { rspq_wait(); }

/* C-side probes.
 *  kind 0: n shaded z-buffered ~20 px triangles submitted straight from C; returns ticks.
 *  kind 4: same with texture + shade + z (24 px legs, the Rust probe's shape).
 *  kind 1: rspq_block_begin (subsequent rdpq commands are recorded, not executed: measures
 *          CPU-only submission cost); kind 2: rspq_block_end; kind 3: run the recorded
 *          block and wait for RSP+RDP idle (returns ticks), then free it. */
static rspq_block_t *probe_block;

uint32_t n64_probe_c_impl(uint32_t kind, uint32_t n) {
    switch (kind) {
    case 0:
    case 4: {
        const n64_state_t st = {N64_ZTEST | N64_ZWRITE | (kind == 4 ? N64_WRAP | N64_BILINEAR : 0), N64_BLEND_NONE,
                                kind == 4 ? 1 : 0, 0};
        apply_state(&st);
        float a[10] = {10, 10, 0.5f, 1, 0, 0, 1, 0, 0, 1};
        float b[10] = {34, 15, 0.5f, 0, 1, 0, 1, 32, 0, 1};
        float c[10] = {17, 34, 0.5f, 0, 0, 1, 1, 0, 32, 1};
        const rdpq_trifmt_t *f = kind == 4 ? &TRIFMT_ZBUF_SHADE_TEX : &TRIFMT_ZBUF_SHADE;
        /* TRIFMT_ZBUF_SHADE (X Y Z R G B A) is a prefix of our 10-float layout. */
        uint32_t t0 = TICKS_READ();
        for (uint32_t i = 0; i < n; i++) rdpq_triangle(f, a, b, c);
        return TICKS_READ() - t0;
    }
    case 1:
        rspq_block_begin();
        return 0;
    case 2:
        probe_block = rspq_block_end();
        return 0;
    case 3: {
        if (!probe_block) return 0;
        rspq_wait();
        uint32_t t0 = TICKS_READ();
        rspq_block_run(probe_block);
        rspq_wait();
        uint32_t t = TICKS_READ() - t0;
        rspq_block_free(probe_block);
        probe_block = NULL;
        return t;
    }
    }
    return 0;
}

int main(void) {
    debug_init_isviewer();
    debug_init_usblog();
    debugf("frogger-n64: libdragon host up, RDRAM %d KiB\n", get_memory_size() / 1024);
    if (get_memory_size() < 8 * 1024 * 1024) debugf("WARNING: no Expansion Pak (8 MiB required)\n");

    display_init(RESOLUTION_320x240, DEPTH_16_BPP, 3, GAMMA_NONE, FILTERS_RESAMPLE);
    zbuf = surface_alloc(FMT_RGBA16, display_get_width(), display_get_height());
    rdpq_init();
    joypad_init();
    if (dfs_init(DFS_DEFAULT_LOCATION) != DFS_ESUCCESS) debugf("DFS: init failed\n");
    asset_init_compression(2);
    asset_init_compression(3);
    audio_init(44100, 4);
    mixer_init(8);

    f2n64_call(f2n64_init);
    for (;;) {
        joypad_poll();
        if (audio_can_write()) mixer_try_play();
        f2n64_call(f2n64_frame);
        mixer_try_play();
    }
}
