/* libdragon host of the Frogger 2 N64 port: boot, display, RDP, audio, joypad, DFS.
 * Rust (libfrogger_n64.a) owns the frame: main() calls f2n64_init() once and f2n64_frame()
 * per frame through the f2n64_call bridge; Rust calls back the n64_* functions below
 * (via the abi.S stubs). Interface and ABI rules: n64_ffi.h. */
#include <libdragon.h>
#include <malloc.h>
#include <string.h>
#include "n64_ffi.h"

#define MAX_TEXTURES 2048
#define MAX_SFX 1024
#define MIXER_CHANNELS 24
#define MUSIC_CH 0 /* stereo: channels 0 and 1 */
#define SFX_CH0 2

static surface_t zbuf;

/* Textures: `full` is the image as uploaded (2D blits split it into TMEM-sized pieces);
 * images wider than the RDP's 1024-texel limit are kept as overlapping column chunks
 * (CHUNK_W wide, CHUNK_STEP apart, so any sub-rect up to 64 texels wide - a glyph of the
 * font strips - lies inside one chunk). `small` fits TMEM (4 KiB) for triangles (== full
 * when full already fits). */
#define CHUNK_W 1024
#define CHUNK_STEP 960
#define MAX_CHUNKS 8
typedef struct {
    surface_t full[MAX_CHUNKS], small;
    int nfull, width;
    bool used, small_is_full;
} tex_t;
static tex_t textures[MAX_TEXTURES]; /* handle 0 = none */
static uint32_t tex_hint = 1;
static bool attached;

/* Last applied RDP mode, to skip redundant mode changes between batches. */
static n64_state_t cur_state;
static int cur_kind = -1; /* 0 = triangles, 1 = rectangles */
static uint32_t cur_tex_loaded;

/* ---- audio: a generated square-wave beep through the mixer (bring-up) ---- */
#define BEEP_CH (MIXER_CHANNELS - 1)
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

/* ---- sound effects (wav64 streamed from DFS) and music ---- */
static wav64_t *sfx[MAX_SFX]; /* handle 0 = none */
static uint32_t sfx_hint = 1;
static wav64_t *music;

static bool file_exists(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return false;
    fclose(f);
    return true;
}

uint32_t n64_sfx_open_impl(const char *path, uint32_t looped) {
    if (!file_exists(path)) return 0;
    for (uint32_t n = 0; n < MAX_SFX - 1; n++) {
        uint32_t h = 1 + (sfx_hint - 1 + n) % (MAX_SFX - 1);
        if (sfx[h]) continue;
        sfx[h] = wav64_load(path, &(wav64_loadparms_t){.streaming_mode = WAV64_STREAMING_FULL});
        if (!sfx[h]) return 0;
        wav64_set_loop(sfx[h], looped != 0);
        sfx_hint = h + 1;
        return h;
    }
    return 0;
}

void n64_sfx_close_impl(uint32_t h) {
    if (h == 0 || h >= MAX_SFX || !sfx[h]) return;
    /* Stop any channel still pulling from this waveform before freeing it. */
    for (int ch = SFX_CH0; ch < BEEP_CH; ch++)
        if (mixer_ch_playing_waveform(ch) == &sfx[h]->wave) mixer_ch_stop(ch);
    wav64_close(sfx[h]);
    sfx[h] = NULL;
    if (h < sfx_hint) sfx_hint = h;
}

uint32_t n64_sfx_channels_impl(void) { return (SFX_CH0 << 8) | (BEEP_CH - SFX_CH0); }

void n64_ch_set_impl(uint32_t ch, const n64_voice_t *v) {
    if (ch >= MIXER_CHANNELS) return;
    mixer_ch_set_vol(ch, v->lvol, v->rvol);
    if (v->freq > 0) mixer_ch_set_freq(ch, v->freq);
}

void n64_sfx_play_impl(uint32_t h, uint32_t ch, const n64_voice_t *v) {
    if (h == 0 || h >= MAX_SFX || !sfx[h] || ch < SFX_CH0 || ch >= BEEP_CH) return;
    mixer_ch_play(ch, &sfx[h]->wave);
    n64_ch_set_impl(ch, v);
}

void n64_ch_stop_impl(uint32_t ch) {
    if (ch < MIXER_CHANNELS) mixer_ch_stop(ch);
}

uint32_t n64_ch_playing_impl(uint32_t ch) { return ch < MIXER_CHANNELS && mixer_ch_playing(ch); }

uint32_t n64_music_play_impl(const char *path, uint32_t looped) {
    if (music) {
        mixer_ch_stop(MUSIC_CH);
        wav64_close(music);
        music = NULL;
    }
    if (!path || !file_exists(path)) return 0;
    music = wav64_load(path, &(wav64_loadparms_t){.streaming_mode = WAV64_STREAMING_FULL});
    if (!music) return 0;
    wav64_set_loop(music, looped != 0);
    wav64_play(music, MUSIC_CH);
    return 1;
}

void n64_music_vol_impl(uint32_t vol_permille) {
    float v = vol_permille / 1000.0f;
    mixer_ch_set_vol(MUSIC_CH, v, v);
}

void n64_audio_poll_impl(void) {
    if (audio_can_write()) mixer_try_play();
}

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

void n64_heap_stats_impl(n64_heap_t *out) {
    heap_stats_t s;
    sys_get_heap_stats(&s);
    out->total = s.total;
    out->used = s.used;
}

uint32_t n64_pad_impl(uint32_t port) {
    if (port > 3) return 0;
    joypad_buttons_t b = joypad_get_buttons(port);
    joypad_inputs_t in = joypad_get_inputs(port);
    return (uint32_t)b.raw | ((uint32_t)(uint8_t)in.stick_x << 16) | ((uint32_t)(uint8_t)in.stick_y << 24);
}

void *n64_file_read_impl(const char *path, uint32_t *len) {
    int sz = 0;
    if (!file_exists(path)) return NULL;
    void *p = asset_load(path, &sz);
    if (len) *len = (uint32_t)sz;
    return p;
}

/* ---- textures ---- */
/* Columns x0 .. x0+w of an image description as a new surface. */
static surface_t make_surface_cols(const n64_tex_desc_t *d, uint32_t x0, uint32_t w) {
    tex_format_t fmt = d->fmt == 1 ? FMT_RGBA32 : FMT_RGBA16;
    surface_t s = surface_alloc(fmt, w, d->height);
    uint32_t row = TEX_FORMAT_PIX2BYTES(fmt, d->width), off = TEX_FORMAT_PIX2BYTES(fmt, x0);
    uint32_t n = TEX_FORMAT_PIX2BYTES(fmt, w);
    for (uint32_t y = 0; y < d->height; y++)
        memcpy((uint8_t *)s.buffer + y * s.stride, (const uint8_t *)d->pixels + y * row + off, n);
    return s;
}

static surface_t make_surface(const n64_tex_desc_t *d) { return make_surface_cols(d, 0, d->width); }

static void free_full(tex_t *t) {
    for (int k = 0; k < t->nfull; k++) surface_free(&t->full[k]);
    t->nfull = 0;
}

uint32_t n64_tex_create2_impl(const n64_tex_desc_t *full, const n64_tex_desc_t *small) {
    for (uint32_t n = 0; n < MAX_TEXTURES - 1; n++) {
        uint32_t h = 1 + (tex_hint - 1 + n) % (MAX_TEXTURES - 1);
        if (textures[h].used) continue;
        tex_t *t = &textures[h];
        t->width = full->width;
        if (full->width <= CHUNK_W) {
            t->full[0] = make_surface(full);
            t->nfull = 1;
        } else {
            t->nfull = 0;
            for (uint32_t x = 0; t->nfull < MAX_CHUNKS && x < full->width; x += CHUNK_STEP) {
                uint32_t w = full->width - x < CHUNK_W ? full->width - x : CHUNK_W;
                t->full[t->nfull++] = make_surface_cols(full, x, w);
                if (x + w >= full->width) break;
            }
        }
        t->small_is_full = small == NULL && t->nfull == 1;
        t->small = small ? make_surface(small) : t->full[0];
        t->used = true;
        tex_hint = h + 1;
        return h;
    }
    return 0;
}

uint32_t n64_tex_create_impl(const n64_tex_desc_t *d) { return n64_tex_create2_impl(d, NULL); }

void n64_tex_free_impl(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return;
    /* The RDP may still read the surfaces of queued commands. */
    rspq_wait();
    tex_t *t = &textures[h];
    if (!t->small_is_full && t->small.buffer != t->full[0].buffer) surface_free(&t->small);
    free_full(t);
    memset(t, 0, sizeof *t);
    if (h < tex_hint) tex_hint = h;
    if (cur_tex_loaded == h) cur_tex_loaded = 0;
}

/* ---- frame ---- */
void n64_begin_frame_impl(uint32_t rgba) {
    surface_t *fb = display_get();
    rdpq_attach(fb, &zbuf);
    rdpq_set_mode_fill(color_from_packed32(rgba));
    rdpq_fill_rectangle(0, 0, display_get_width(), display_get_height());
    rdpq_clear_z(ZBUF_MAX);
    attached = true;
    cur_kind = -1;
    cur_tex_loaded = 0;
}

static void apply_state(const n64_state_t *st, int kind) {
    bool tex = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used;
    if (cur_kind != kind || st->flags != cur_state.flags || st->blend != cur_state.blend ||
        st->alpha_ref != cur_state.alpha_ref || (st->tex == 0) != (cur_state.tex == 0)) {
        rdpq_set_mode_standard();
        if (kind == 0)
            rdpq_mode_combiner(tex ? RDPQ_COMBINER_TEX_SHADE : RDPQ_COMBINER_SHADE);
        else
            rdpq_mode_combiner(tex ? RDPQ_COMBINER_TEX_FLAT : RDPQ_COMBINER_FLAT);
        rdpq_mode_zbuf(st->flags & N64_ZTEST, st->flags & N64_ZWRITE);
        rdpq_mode_filter(st->flags & N64_BILINEAR ? FILTER_BILINEAR : FILTER_POINT);
        rdpq_mode_persp(kind == 0 && (st->flags & N64_PERSP));
        rdpq_mode_alphacompare((int)st->alpha_ref);
        switch (st->blend) {
        case N64_BLEND_SEMI: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        case N64_BLEND_ADD: rdpq_mode_blender(RDPQ_BLENDER_ADDITIVE); break;
        /* TODO(phase 2): D3D ZERO/INVSRCCOLOR has no exact RDP blender equivalent (the
         * blender only has alpha factors); approximated as multiply for now. */
        case N64_BLEND_SUB: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        default: rdpq_mode_blender(0); break;
        }
        cur_tex_loaded = 0;
    }
    if (kind == 0 && tex && (st->tex != cur_tex_loaded || ((st->flags ^ cur_state.flags) & N64_WRAP))) {
        rdpq_texparms_t parms = {0};
        if (st->flags & N64_WRAP) parms.s.repeats = parms.t.repeats = REPEAT_INFINITE;
        rdpq_tex_upload(TILE0, &textures[st->tex].small, &parms);
        cur_tex_loaded = st->tex;
    }
    if (kind == 1) cur_tex_loaded = 0; /* blits reload TMEM */
    cur_state = *st;
    cur_kind = kind;
}

void n64_draw_tris_impl(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    if (!attached || !ntris) return;
    apply_state(st, 0);
    bool tex = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used;
    /* Our vertices always have the 10-float ZBUF_SHADE_TEX layout; describe the subset used. */
    static rdpq_trifmt_t fmts[4];
    int i = (tex ? 1 : 0) | ((st->flags & (N64_ZTEST | N64_ZWRITE)) ? 2 : 0);
    rdpq_trifmt_t *f = &fmts[i];
    *f = (rdpq_trifmt_t){
        .pos_offset = 0, .z_offset = (i & 2) ? 2 : -1, .shade_offset = 3,
        .tex_offset = (i & 1) ? 7 : -1, .tex_tile = TILE0,
    };
    if (tex && (st->flags & N64_UVNORM)) {
        /* Normalised UVs: scale to the TMEM-sized surface. */
        const surface_t *s = &textures[st->tex].small;
        float sw = s->width, sh = s->height;
        n64_vertex_t t[3];
        for (uint32_t k = 0; k < ntris; k++, v += 3) {
            for (int j = 0; j < 3; j++) {
                t[j] = v[j];
                t[j].s *= sw;
                t[j].t *= sh;
            }
            rdpq_triangle(f, (const float *)&t[0], (const float *)&t[1], (const float *)&t[2]);
        }
        return;
    }
    for (uint32_t k = 0; k < ntris; k++, v += 3)
        rdpq_triangle(f, (const float *)&v[0], (const float *)&v[1], (const float *)&v[2]);
}

void n64_draw_rects_impl(const n64_rect_t *r, uint32_t n, const n64_state_t *st) {
    if (!attached || !n) return;
    apply_state(st, 1);
    bool tex = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used;
    const tex_t *tx = tex ? &textures[st->tex] : NULL;
    for (uint32_t k = 0; k < n; k++, r++) {
        rdpq_set_prim_color(color_from_packed32(r->rgba));
        if (!tex) {
            rdpq_fill_rectangle(r->x0, r->y0, r->x1, r->y1);
            continue;
        }
        const surface_t *s = &tx->full[0];
        float w = tx->width, h = s->height;
        float s0 = r->s0 * w, s1 = r->s1 * w, t0 = r->t0 * h, t1 = r->t1 * h;
        bool fx = s1 < s0, fy = t1 < t0;
        float sa = fx ? s1 : s0, sb = fx ? s0 : s1, ta = fy ? t1 : t0, tb = fy ? t0 : t1;
        int is0 = (int)(sa + 0.5f), it0 = (int)(ta + 0.5f);
        int is1 = (int)(sb + 0.5f), it1 = (int)(tb + 0.5f);
        if (tx->nfull > 1) {
            /* Wide image: the chunk holding the sub-rect, in chunk coordinates. */
            int c = is0 < 0 ? 0 : is0 / CHUNK_STEP;
            if (c >= tx->nfull) c = tx->nfull - 1;
            s = &tx->full[c];
            is0 -= c * CHUNK_STEP;
            is1 -= c * CHUNK_STEP;
        }
        if (is0 < 0) is0 = 0;
        if (it0 < 0) it0 = 0;
        if (is1 > s->width) is1 = s->width;
        if (it1 > s->height) it1 = s->height;
        int sw = is1 - is0, sh = it1 - it0;
        if (sw <= 0 || sh <= 0 || r->x1 <= r->x0 || r->y1 <= r->y0) continue;
        rdpq_tex_blit(s, r->x0, r->y0, &(rdpq_blitparms_t){
            .s0 = is0, .t0 = it0, .width = sw, .height = sh, .flip_x = fx, .flip_y = fy,
            .scale_x = (r->x1 - r->x0) / sw, .scale_y = (r->y1 - r->y0) / sh,
            .filtering = (st->flags & N64_BILINEAR) != 0,
        });
    }
}

void n64_present_impl(void) {
    if (!attached) return;
    rdpq_detach_show();
    attached = false;
}

void n64_rdp_wait_impl(void) { rspq_wait(); }

/* ---- sampling profiler (feature `profile`): a 2 kHz timer interrupt records the
 * interrupted PC (and $ra) of the main thread in 32-byte buckets; n64_prof(1) prints the
 * hottest buckets (map them to functions with mips64-elf-addr2line -f -e build/frogger2.elf)
 * and resets. ---- */
#define PROF_BASE 0x80000000u
#define PROF_SHIFT 5
#define PROF_BUCKETS (0x200000u >> PROF_SHIFT) /* first 2 MiB of RDRAM: the code */
extern void *interrupt_exception_frame;
static uint16_t *prof_pc, *prof_ra;
static uint32_t prof_samples, prof_other;
static volatile bool prof_paused;
static timer_link_t *prof_timer;

static void prof_cb(int ovfl) {
    uint8_t *f = (uint8_t *)interrupt_exception_frame;
    if (!f || !prof_pc || prof_paused) return;
    uint32_t epc = *(uint32_t *)(f + 312);       /* STACK_EPC (inthandler.S) */
    uint32_t ra = *(uint32_t *)(f + 32 + 31 * 8 + 4); /* gpr[31] low word */
    prof_samples++;
    uint32_t i = (epc - PROF_BASE) >> PROF_SHIFT, j = (ra - PROF_BASE) >> PROF_SHIFT;
    if (i < PROF_BUCKETS) { if (prof_pc[i] < 0xffff) prof_pc[i]++; } else prof_other++;
    if (j < PROF_BUCKETS && prof_ra[j] < 0xffff) prof_ra[j]++;
}

static void prof_top(const char *what, uint16_t *h) {
    for (int k = 0; k < 400; k++) {
        uint32_t best = 0, bi = 0;
        for (uint32_t i = 0; i < PROF_BUCKETS; i++)
            if (h[i] > best) { best = h[i]; bi = i; }
        if (!best) break;
        debugf("PROF %s %08lx %lu\n", what, (unsigned long)(PROF_BASE + (bi << PROF_SHIFT)), (unsigned long)best);
        h[bi] = 0;
    }
}

void n64_prof_impl(uint32_t cmd) {
    if (cmd == 2 || cmd == 3) { /* 2 = pause, 3 = resume sampling */
        prof_paused = cmd == 2;
        return;
    }
    if (cmd == 0 && !prof_pc) {
        prof_pc = calloc(PROF_BUCKETS, 2);
        prof_ra = calloc(PROF_BUCKETS, 2);
        timer_init();
        prof_timer = new_timer(TIMER_TICKS(500), TF_CONTINUOUS, prof_cb);
    } else if (cmd == 1 && prof_pc) {
        stop_timer(prof_timer);
        debugf("PROF samples %lu (outside code %lu)\n", (unsigned long)prof_samples, (unsigned long)prof_other);
        prof_top("pc", prof_pc);
        prof_top("ra", prof_ra);
        memset(prof_pc, 0, PROF_BUCKETS * 2);
        memset(prof_ra, 0, PROF_BUCKETS * 2);
        prof_samples = prof_other = 0;
        start_timer(prof_timer, TIMER_TICKS(500), TF_CONTINUOUS, prof_cb);
    }
}

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
        apply_state(&st, 0);
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
    /* Generous headroom: the game may render well below 60 fps; Rust also polls the mixer
     * between simulation ticks and during loads (n64_audio_poll). */
    audio_init(32000, AUDIO_INIT_LATENCY_MS(500));
    mixer_init(MIXER_CHANNELS);

    f2n64_call(f2n64_init);
    for (;;) {
        joypad_poll();
        n64_audio_poll_impl();
        f2n64_call(f2n64_frame);
        n64_audio_poll_impl();
    }
}
