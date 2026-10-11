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
static surface_t *last_fb;

/* Textures: `full` is the image as uploaded (2D blits split it into TMEM-sized pieces);
 * images wider than the RDP's 1024-texel limit are kept as overlapping column chunks
 * (CHUNK_W wide, CHUNK_STEP apart, so any sub-rect up to 64 texels wide - a glyph of the
 * font strips - lies inside one chunk). `small` fits TMEM (4 KiB) for triangles (== full
 * when full already fits). */
#define CHUNK_W 1024
#define CHUNK_STEP 960
#define MAX_CHUNKS 8
typedef struct {
    surface_t *full;    /* &full1, or a malloc'd array of MAX_CHUNKS column chunks */
    surface_t full1, small;
    int nfull, width;
    bool used, small_is_full;
    bool tile3d; /* triangles sample `full` in TMEM tiles instead of `small` (N64_TEX_TILE3D) */
    surface_t addv; /* additive variant (made on first additive use, see make_add_variant) */
    bool has_addv;
    /* NTX textures (n64_tex_create_ntx): full[0] points into the caller's file buffer. */
    bool external;
    uint16_t *tlut;     /* RGBA16 palette (CI formats), NULL = none */
    int tlut_count;
    bool per_tile;      /* one 256-entry palette per tile_w x tile_h tile (backdrops) */
    int tile_w, tile_h;
    bool own_small;     /* `small` is a downscaled copy (made on demand, see make_small_variant) */
    surface_t tmem;     /* r3d: downscaled copy of a texture larger than one TMEM load */
    bool has_tmem;
} tex_t;
static int cur_tlut_mode = -1; /* TLUT mode currently set in the SOM (-1 = unknown) */
static void make_add_variant(tex_t *t);
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

/* Native playback rate of an opened sample (Hz; Opus wav64 files are stored at 48 kHz). */
uint32_t n64_sfx_freq_impl(uint32_t h) {
    if (h == 0 || h >= MAX_SFX || !sfx[h]) return 0;
    return (uint32_t)sfx[h]->wave.frequency;
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

/* ---- FMV (libdragon fmv_play: H.264 + Opus wav64 next to it) ---- */
#define FMV_CH 0 /* the music channel pair (the game stops its music around videos) */

static void fmv_osd(void *ctx, int frame_idx, float time_sec, fmv_control_t *ctrl) {
    /* Skippable with A / B / Start like the PC's key press. */
    joypad_poll();
    joypad_buttons_t b = joypad_get_buttons_pressed(JOYPAD_PORT_1);
    if (frame_idx > 2 && (b.a || b.b || b.start)) ctrl->stop(ctrl);
}

/* Plays a video over the game's 320x240 16-bit display (no display re-init: the game's
 * framebuffers stay); returns 0 if the file is missing. */
uint32_t n64_fmv_play_impl(const char *path) {
    if (!file_exists(path)) return 0;
    rspq_wait();
    if (music) {
        mixer_ch_stop(MUSIC_CH);
        wav64_close(music);
        music = NULL;
    }
    fmv_parms_t p = {
        .disable_display_init = true,
        .disable_subtitles = true,
        .audio_mixer_channel = FMV_CH,
        .decode_ahead_pics = 4,
        .osd_callback = fmv_osd,
    };
    fmv_play(path, &p);
    display_set_fps_limit(0);
    mixer_ch_set_limits(FMV_CH, 16, 48000, 0);
    mixer_ch_set_limits(FMV_CH + 1, 16, 48000, 0);
    cur_kind = -1;
    cur_tex_loaded = 0;
    cur_tlut_mode = -1;
    return 1;
}

/* ---- saves: cartridge SRAM (256 Kbit) ---- */
static bool sram_ok;

uint32_t n64_save_read_impl(void *dst, uint32_t len) {
    if (!sram_ok || len > 0x8000) return 0;
    return sram_read(dst, 0, len) == (int)len ? len : 0;
}

uint32_t n64_save_write_impl(const void *src, uint32_t len) {
    if (!sram_ok || len > 0x8000) return 0;
    return sram_write(src, 0, len) == (int)len ? len : 0;
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
    tex_format_t fmt = (d->fmt & 0xff) == 1 ? FMT_RGBA32 : FMT_RGBA16;
    surface_t s = surface_alloc(fmt, w, d->height);
    uint32_t row = TEX_FORMAT_PIX2BYTES(fmt, d->width), off = TEX_FORMAT_PIX2BYTES(fmt, x0);
    uint32_t n = TEX_FORMAT_PIX2BYTES(fmt, w);
    for (uint32_t y = 0; y < d->height; y++)
        memcpy((uint8_t *)s.buffer + y * s.stride, (const uint8_t *)d->pixels + y * row + off, n);
    return s;
}

static surface_t make_surface(const n64_tex_desc_t *d) { return make_surface_cols(d, 0, d->width); }

static void free_full(tex_t *t) {
    if (!t->external)
        for (int k = 0; k < t->nfull; k++) surface_free(&t->full[k]);
    if (t->full && t->full != &t->full1) free(t->full);
    t->full = NULL;
    t->nfull = 0;
}

/* Storage for a texture's full surface(s): inline for one, an array for column chunks. */
static void alloc_full(tex_t *t, bool chunks) {
    t->full = chunks ? calloc(MAX_CHUNKS, sizeof(surface_t)) : &t->full1;
}

uint32_t n64_tex_create2_impl(const n64_tex_desc_t *full, const n64_tex_desc_t *small) {
    for (uint32_t n = 0; n < MAX_TEXTURES - 1; n++) {
        uint32_t h = 1 + (tex_hint - 1 + n) % (MAX_TEXTURES - 1);
        if (textures[h].used) continue;
        tex_t *t = &textures[h];
        t->width = full->width;
        alloc_full(t, full->width > CHUNK_W);
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
        t->tile3d = (full->fmt & N64_TEX_TILE3D) && t->nfull == 1 && small != NULL;
        t->used = true;
        tex_hint = h + 1;
        return h;
    }
    return 0;
}

uint32_t n64_tex_create_impl(const n64_tex_desc_t *d) { return n64_tex_create2_impl(d, NULL); }

static tex_format_t ntx_format(uint32_t f) {
    static const tex_format_t fmts[9] = {FMT_RGBA16, FMT_RGBA32, FMT_CI4, FMT_CI8, FMT_IA4, FMT_IA8, FMT_I4, FMT_I8, FMT_IA16};
    return f < 9 ? fmts[f] : FMT_NONE;
}

/* A texture over texels in the caller's buffer (an NTX1 image of the 1C pack): no copy;
 * the buffer must stay alive until the texture is freed. Images wider than the RDP's 1024
 * texels are copied into column chunks. Not SINGLE images (2D, larger than TMEM) are drawn
 * on triangles in TMEM tiles. */
uint32_t n64_tex_create_ntx_impl(const n64_ntx_desc_t *d) {
    tex_format_t fmt = ntx_format(d->fmt);
    if (fmt == FMT_NONE || !d->width || !d->height) return 0;
    for (uint32_t n = 0; n < MAX_TEXTURES - 1; n++) {
        uint32_t h = 1 + (tex_hint - 1 + n) % (MAX_TEXTURES - 1);
        if (textures[h].used) continue;
        tex_t *t = &textures[h];
        memset(t, 0, sizeof *t);
        t->width = d->width;
        alloc_full(t, d->width > CHUNK_W);
        data_cache_hit_writeback(d->texels, d->stride * d->height);
        if (d->width <= CHUNK_W) {
            t->full[0] = surface_make((void *)d->texels, fmt, d->width, d->height, d->stride);
            t->nfull = 1;
            t->external = true;
        } else {
            for (uint32_t x = 0; t->nfull < MAX_CHUNKS && x < d->width; x += CHUNK_STEP) {
                uint32_t w = d->width - x < CHUNK_W ? d->width - x : CHUNK_W;
                surface_t c = surface_alloc(fmt, w, d->height);
                uint32_t off = TEX_FORMAT_PIX2BYTES(fmt, x), nb = TEX_FORMAT_PIX2BYTES(fmt, w);
                for (uint32_t y = 0; y < d->height; y++)
                    memcpy((uint8_t *)c.buffer + y * c.stride, (const uint8_t *)d->texels + y * d->stride + off, nb);
                t->full[t->nfull++] = c;
                if (x + w >= d->width) break;
            }
        }
        if (d->tlut && d->tlut_count) {
            t->tlut = (uint16_t *)d->tlut;
            t->tlut_count = d->tlut_count;
            data_cache_hit_writeback(d->tlut, d->tlut_count * 2);
        }
        t->per_tile = (d->flags & N64_NTX_PER_TILE) != 0;
        t->tile_w = d->tile_w;
        t->tile_h = d->tile_h;
        t->small = t->full[0];
        t->small_is_full = true;
        t->tile3d = !(d->flags & N64_NTX_SINGLE) && t->nfull == 1;
        t->used = true;
        tex_hint = h + 1;
        return h;
    }
    return 0;
}

/* Replaces the texels of texture h in place (same size and format, single surface): the
 * procedural textures (water, fire) change every frame. No reallocation, so r3d aliases and
 * their load descriptors stay valid; the RDP may still be reading the previous texels for an
 * in-flight frame (a one-frame mix at worst). Returns 0 if h cannot be updated in place. */
uint32_t n64_tex_update_impl(uint32_t h, const n64_tex_desc_t *d) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return 0;
    tex_t *t = &textures[h];
    tex_format_t fmt = (d->fmt & 0xff) == 1 ? FMT_RGBA32 : FMT_RGBA16;
    if (!t->small_is_full || t->nfull != 1 || t->tile3d || t->external || t->own_small) return 0;
    surface_t *s = &t->full[0];
    if (s->width != d->width || s->height != d->height || surface_get_format(s) != fmt) return 0;
    uint32_t row = TEX_FORMAT_PIX2BYTES(fmt, d->width);
    for (uint32_t y = 0; y < d->height; y++)
        memcpy((uint8_t *)s->buffer + y * s->stride, (const uint8_t *)d->pixels + y * row, row);
    /* surface_alloc buffers are uncached; others need a writeback for the RDP. */
    if (!((uint32_t)s->buffer & 0x20000000)) data_cache_hit_writeback(s->buffer, s->stride * s->height);
    if (t->has_addv) make_add_variant(t);
    if (cur_tex_loaded == h) cur_tex_loaded = 0;
    return 1;
}

/* The TMEM-sized surface triangles of texture h sample (r3d aliases it), NULL if none. */
const surface_t *n64_tex_surface(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return NULL;
    return &textures[h].small;
}

/* Forget the cached RDP mode/texture (another renderer - r3d - changed the RDP state). */
void n64_state_invalidate(void) {
    cur_kind = -1;
    cur_tex_loaded = 0;
    cur_tlut_mode = -1;
}

/* The palette of texture t (NULL if none) for TLUT uploads by r3d. */
const uint16_t *n64_tex_tlut(uint32_t h, uint32_t *count) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used || !textures[h].tlut) return NULL;
    *count = textures[h].tlut_count;
    return textures[h].tlut;
}

void n64_tex_free_impl(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return;
    /* The RDP may still read the surfaces of queued commands. */
    rspq_wait();
    tex_t *t = &textures[h];
    if ((t->own_small || !t->small_is_full) && t->small.buffer != t->full[0].buffer) surface_free(&t->small);
    if (t->has_addv) surface_free(&t->addv);
    if (t->has_tmem) surface_free(&t->tmem);
    free_full(t);
    memset(t, 0, sizeof *t);
    if (h < tex_hint) tex_hint = h;
    if (cur_tex_loaded == h) cur_tex_loaded = 0;
}

/* ---- frame ---- */
void n64_begin_frame_impl(uint32_t rgba) {
    surface_t *fb = display_get();
    last_fb = fb;
    rdpq_attach(fb, &zbuf);
    rdpq_set_mode_fill(color_from_packed32(rgba));
    rdpq_fill_rectangle(0, 0, display_get_width(), display_get_height());
    rdpq_clear_z(ZBUF_MAX);
    attached = true;
    cur_kind = -1;
    cur_tex_loaded = 0;
    cur_tlut_mode = -1;
}

/* ---- additive blending without wrap-around ----
 * The RDP blender computes P*A + M*B; the D3D additive mode (SRCALPHA, ONE) maps to
 * IN*IN_ALPHA + MEM*1, whose sum WRAPS past 1.0 instead of saturating (libdragon:
 * "additive blending is broken on RDP"): bright water caustics over the dark river turned
 * into red/green splotches. There is no saturating additive factor, so additive batches are
 * drawn with the alpha blender (IN*a + MEM*(1-a), which cannot overflow) from a variant of
 * the texture whose colour is normalised to its brightest channel and whose alpha carries
 * that brightness: c' = rgb / max(rgb), a' = a * max(rgb). Then IN*a' equals the additive
 * term rgb*a exactly; the only difference to D3D is that the background is attenuated by
 * (1 - a') where the effect is bright. Variants are RGBA32 (8-bit alpha), at most 1024
 * texels (box-halved if needed), made once per texture on its first additive use. */
/* RGBA8 of texel (x, y) of surface s of texture t (any format). */
static void tex_fetch(const tex_t *t, const surface_t *s, int x, int y, uint32_t px[4]) {
    const uint8_t *row = (const uint8_t *)s->buffer + y * s->stride;
    uint32_t v;
    switch (surface_get_format(s)) {
    case FMT_RGBA32: px[0] = row[x * 4]; px[1] = row[x * 4 + 1]; px[2] = row[x * 4 + 2]; px[3] = row[x * 4 + 3]; return;
    case FMT_CI4: v = (row[x / 2] >> ((x & 1) ? 0 : 4)) & 15; goto pal;
    case FMT_CI8: v = row[x];
    pal:
        v = t->tlut && (int)v < t->tlut_count ? t->tlut[v] : 0;
        goto rgba16;
    case FMT_IA4: v = (row[x / 2] >> ((x & 1) ? 0 : 4)) & 15;
        px[0] = px[1] = px[2] = ((v >> 1) << 5) | ((v >> 1) << 2) | ((v >> 1) >> 1); px[3] = (v & 1) ? 255 : 0; return;
    case FMT_IA8: v = row[x]; px[0] = px[1] = px[2] = (v >> 4) * 17; px[3] = (v & 15) * 17; return;
    case FMT_IA16: px[0] = px[1] = px[2] = row[x * 2]; px[3] = row[x * 2 + 1]; return;
    case FMT_I4: v = (row[x / 2] >> ((x & 1) ? 0 : 4)) & 15; px[0] = px[1] = px[2] = px[3] = v * 17; return;
    case FMT_I8: px[0] = px[1] = px[2] = px[3] = row[x]; return;
    default: v = (row[x * 2] << 8) | row[x * 2 + 1];
    rgba16:
        px[0] = ((v >> 11) & 31) * 255 / 31; px[1] = ((v >> 6) & 31) * 255 / 31; px[2] = ((v >> 1) & 31) * 255 / 31;
        px[3] = (v & 1) ? 255 : 0;
        return;
    }
}

/* A box-downscaled RGBA32/RGBA16 copy of a texture that does not fit TMEM, for the triangle
 * cases the tile path does not cover (wrapping). */
static surface_t downscale_rgba32(tex_t *t, const surface_t *s);

static void make_small_variant(tex_t *t) {
    t->small = downscale_rgba32(t, &t->full[0]);
    t->own_small = true;
    t->small_is_full = false;
}

/* Whether surface s loads into TMEM with one LOAD_BLOCK (r3d's texture loads). */
static bool fits_tmem_block(const surface_t *s) {
    tex_format_t f = surface_get_format(s);
    uint32_t limit = (f == FMT_CI4 || f == FMT_CI8) ? 2048 : 4096;
    return !(s->width & (s->width - 1)) && !(s->height & (s->height - 1)) && s->stride == TEX_FORMAT_PIX2BYTES(f, s->width) &&
           s->stride >= 8 && s->stride * s->height <= limit;
}

/* The surface r3d samples for texture h: `small` when it fits one TMEM load, else a
 * downscaled RGBA32 copy (made once). NULL if none. */
const surface_t *n64_tex_tmem_surface(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return NULL;
    tex_t *t = &textures[h];
    if (fits_tmem_block(&t->small)) return &t->small;
    if (!t->has_tmem) {
        t->tmem = downscale_rgba32(t, &t->small);
        t->has_tmem = true;
    }
    return &t->tmem;
}

/* A box-downscaled RGBA32 copy of surface s of texture t, at most 1024 texels. */
static surface_t downscale_rgba32(tex_t *t, const surface_t *s) {
    int w = s->width, h = s->height, sx = 1, sy = 1;
    while ((w / sx) * (h / sy) > 1024) {
        if (w / sx >= h / sy) sx *= 2; else sy *= 2;
    }
    int nw = w / sx > 0 ? w / sx : 1, nh = h / sy > 0 ? h / sy : 1;
    surface_t d = surface_alloc(FMT_RGBA32, nw, nh);
    for (int y = 0; y < nh; y++)
        for (int x = 0; x < nw; x++) {
            uint32_t acc[4] = {0, 0, 0, 0}, n = 0, px[4];
            for (int yy = y * sy; yy < (y + 1) * sy && yy < h; yy++)
                for (int xx = x * sx; xx < (x + 1) * sx && xx < w; xx++) {
                    tex_fetch(t, s, xx, yy, px);
                    for (int k = 0; k < 4; k++) acc[k] += px[k];
                    n++;
                }
            uint8_t *o = (uint8_t *)d.buffer + y * d.stride + x * 4;
            for (int k = 0; k < 4; k++) o[k] = n ? acc[k] / n : 0;
        }
    return d;
}

static void make_add_variant(tex_t *t) {
    const surface_t *s = &t->small;
    int w = s->width, h = s->height;
    int sx = 1, sy = 1;
    while ((w / sx) * (h / sy) > 1024) {
        if (w / sx >= h / sy) sx *= 2; else sy *= 2;
    }
    int nw = w / sx > 0 ? w / sx : 1, nh = h / sy > 0 ? h / sy : 1;
    /* Refilled in place when the texture changes (procedural textures): r3d aliases it. */
    if (!t->has_addv || t->addv.width != nw || t->addv.height != nh) {
        if (t->has_addv) surface_free(&t->addv);
        t->addv = surface_alloc(FMT_RGBA32, nw, nh);
    }
    for (int y = 0; y < nh; y++) {
        uint8_t *dst = (uint8_t *)t->addv.buffer + y * t->addv.stride;
        for (int x = 0; x < nw; x++) {
            uint32_t acc[4] = {0, 0, 0, 0}, n = 0;
            for (int yy = y * sy; yy < (y + 1) * sy && yy < h; yy++)
                for (int xx = x * sx; xx < (x + 1) * sx && xx < w; xx++) {
                    uint32_t px[4];
                    tex_fetch(t, s, xx, yy, px);
                    uint32_t r = px[0], g = px[1], b = px[2], a = px[3];
                    /* premultiplied accumulation: what the additive mode adds is rgb*a */
                    acc[0] += r * a; acc[1] += g * a; acc[2] += b * a; n++;
                }
            uint32_t r = acc[0] / (255 * n), g = acc[1] / (255 * n), b = acc[2] / (255 * n);
            uint32_t m = r > g ? (r > b ? r : b) : (g > b ? g : b);
            dst[x * 4 + 0] = m ? r * 255 / m : 0;
            dst[x * 4 + 1] = m ? g * 255 / m : 0;
            dst[x * 4 + 2] = m ? b * 255 / m : 0;
            dst[x * 4 + 3] = m;
        }
    }
    t->has_addv = true;
}

/* The surface triangles of this state sample (TMEM-sized). */
static const surface_t *tri_surface(const n64_state_t *st) {
    tex_t *t = &textures[st->tex];
    if (st->blend == N64_BLEND_ADD) {
        if (!t->has_addv) make_add_variant(t);
        return &t->addv;
    }
    if (t->small_is_full && t->tile3d && !t->own_small) make_small_variant(t);
    return &t->small;
}

/* The additive variant surface of texture h (made on first use; see make_add_variant). */
const surface_t *n64_tex_add_surface(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return NULL;
    tex_t *t = &textures[h];
    if (!t->has_addv) make_add_variant(t);
    return &t->addv;
}

/* Palette mode + TLUT for sampling surface s of texture t (tile `tile` of per-tile ones). */
static void use_tlut(const tex_t *t, const surface_t *s, int tile) {
    tex_format_t f = surface_get_format(s);
    int want = (f == FMT_CI4 || f == FMT_CI8) ? 1 : 0;
    if (want != cur_tlut_mode) {
        rdpq_mode_tlut(want ? TLUT_RGBA16 : TLUT_NONE);
        cur_tlut_mode = want;
    }
    if (want && t->tlut) {
        int n = f == FMT_CI4 ? 16 : (t->per_tile ? 256 : t->tlut_count);
        rdpq_tex_upload_tlut(t->tlut + (t->per_tile ? 256 * tile : 0), 0, n > 256 ? 256 : n);
    }
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
        /* Never RDPQ_BLENDER_ADDITIVE (wraps): see make_add_variant. */
        case N64_BLEND_ADD: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        /* TODO(phase 2): D3D ZERO/INVSRCCOLOR has no exact RDP blender equivalent (the
         * blender only has alpha factors); approximated as multiply for now. */
        case N64_BLEND_SUB: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        default: rdpq_mode_blender(0); break;
        }
        cur_tex_loaded = 0;
        cur_tlut_mode = 0; /* rdpq_set_mode_standard: no TLUT */
    }
    if (kind == 0 && tex && (st->tex != cur_tex_loaded || ((st->flags ^ cur_state.flags) & N64_WRAP))) {
        rdpq_texparms_t parms = {0};
        if (st->flags & N64_WRAP) parms.s.repeats = parms.t.repeats = REPEAT_INFINITE;
        const surface_t *s = tri_surface(st);
        use_tlut(&textures[st->tex], s, 0);
        rdpq_tex_upload(TILE0, s, &parms);
        cur_tex_loaded = st->tex;
    }
    if (kind == 1) cur_tex_loaded = 0; /* blits reload TMEM */
    cur_state = *st;
    cur_kind = kind;
}

/* ---- large textures on triangles: TMEM tiles ----
 * A triangle whose texture does not fit TMEM is clipped (in texel space) against a grid of
 * TILE_W x TILE_H texel tiles; each piece is drawn after uploading its tile (+1 texel border
 * for bilinear) with rdpq_tex_upload_sub, which keeps the full texture's coordinates.
 * Attributes are interpolated linearly (the triangles are affine). Used for textures
 * flagged N64_TEX_TILE3D (render-to-texture pages, fonts, screens), not for wrapping ones. */
#define TILE_W 32
#define TILE_H 32
#define CLIP_MAX 12

static int clip_axis(n64_vertex_t *in, int n, n64_vertex_t *out, int axis, float lim, bool keep_greater) {
    int m = 0;
    for (int i = 0; i < n; i++) {
        n64_vertex_t *a = &in[i], *b = &in[(i + 1) % n];
        float va = axis ? a->t : a->s, vb = axis ? b->t : b->s;
        bool ia = keep_greater ? va >= lim : va <= lim, ib = keep_greater ? vb >= lim : vb <= lim;
        if (ia && m < CLIP_MAX) out[m++] = *a;
        if (ia != ib && m < CLIP_MAX) {
            float k = (lim - va) / (vb - va);
            const float *fa = (const float *)a, *fb = (const float *)b;
            float *fo = (float *)&out[m++];
            for (int j = 0; j < 10; j++) fo[j] = fa[j] + (fb[j] - fa[j]) * k;
        }
    }
    return m;
}

static void draw_tri_tiled(const rdpq_trifmt_t *f, const tex_t *tx, const n64_vertex_t *v, uint32_t flags) {
    const surface_t *s = &tx->full[0];
    float w = s->width, h = s->height;
    n64_vertex_t t[3];
    float smin = 1e9f, smax = -1e9f, tmin = 1e9f, tmax = -1e9f;
    for (int j = 0; j < 3; j++) {
        t[j] = v[j];
        t[j].s *= w;
        t[j].t *= h;
        if (t[j].s < smin) smin = t[j].s;
        if (t[j].s > smax) smax = t[j].s;
        if (t[j].t < tmin) tmin = t[j].t;
        if (t[j].t > tmax) tmax = t[j].t;
    }
    int x0 = (int)smin, x1 = (int)smax + 1, y0 = (int)tmin, y1 = (int)tmax + 1;
    if (x0 < 0) x0 = 0;
    if (y0 < 0) y0 = 0;
    if (x1 > (int)w) x1 = w;
    if (y1 > (int)h) y1 = h;
    rdpq_texparms_t parms = {0};
    use_tlut(tx, s, 0);
    for (int ty = y0 / TILE_H * TILE_H; ty < y1; ty += TILE_H) {
        for (int tx0 = x0 / TILE_W * TILE_W; tx0 < x1; tx0 += TILE_W) {
            n64_vertex_t a[CLIP_MAX], b[CLIP_MAX];
            int n = 3;
            memcpy(a, t, sizeof t);
            /* Outer tiles keep what lies beyond the texture (clamped addressing). */
            if (tx0 > 0) n = clip_axis(a, n, b, 0, tx0, true); else memcpy(b, a, sizeof(n64_vertex_t) * n);
            if (n < 3) continue;
            if (tx0 + TILE_W < (int)w) n = clip_axis(b, n, a, 0, tx0 + TILE_W, false); else memcpy(a, b, sizeof(n64_vertex_t) * n);
            if (n < 3) continue;
            if (ty > 0) n = clip_axis(a, n, b, 1, ty, true); else memcpy(b, a, sizeof(n64_vertex_t) * n);
            if (n < 3) continue;
            if (ty + TILE_H < (int)h) n = clip_axis(b, n, a, 1, ty + TILE_H, false); else memcpy(a, b, sizeof(n64_vertex_t) * n);
            if (n < 3) continue;
            int ux1 = tx0 + TILE_W + 1, uy1 = ty + TILE_H + 1;
            if (ux1 > (int)w) ux1 = w;
            if (uy1 > (int)h) uy1 = h;
            rdpq_tex_upload_sub(TILE0, s, &parms, tx0, ty, ux1, uy1);
            for (int k = 1; k + 1 < n; k++)
                rdpq_triangle(f, (const float *)&a[0], (const float *)&a[k], (const float *)&a[k + 1]);
        }
    }
    cur_tex_loaded = 0;
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
    if (tex && (st->flags & N64_UVNORM) && textures[st->tex].tile3d && !textures[st->tex].own_small && !(st->flags & N64_WRAP) && st->blend != N64_BLEND_ADD) {
        for (uint32_t k = 0; k < ntris; k++, v += 3) draw_tri_tiled(f, &textures[st->tex], v, st->flags);
        return;
    }
    if (tex && (st->flags & N64_UVNORM)) {
        /* Normalised UVs: scale to the TMEM-sized surface. */
        const surface_t *s = tri_surface(st);
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
        if (tx->per_tile && tx->tile_w > 0 && tx->tile_h > 0 && !fx && !fy) {
            /* One palette per tile: blit tile by tile (unfiltered, as the backdrops are). */
            float kx = (r->x1 - r->x0) / sw, ky = (r->y1 - r->y0) / sh;
            int ntx = (s->width + tx->tile_w - 1) / tx->tile_w;
            for (int ty = it0 / tx->tile_h * tx->tile_h; ty < it1; ty += tx->tile_h)
                for (int tx0 = is0 / tx->tile_w * tx->tile_w; tx0 < is1; tx0 += tx->tile_w) {
                    int a0 = tx0 > is0 ? tx0 : is0, b0 = ty > it0 ? ty : it0;
                    int a1 = tx0 + tx->tile_w < is1 ? tx0 + tx->tile_w : is1, b1 = ty + tx->tile_h < it1 ? ty + tx->tile_h : it1;
                    if (a1 <= a0 || b1 <= b0) continue;
                    use_tlut(tx, s, (ty / tx->tile_h) * ntx + tx0 / tx->tile_w);
                    rdpq_tex_blit(s, r->x0 + (a0 - is0) * kx, r->y0 + (b0 - it0) * ky, &(rdpq_blitparms_t){
                        .s0 = a0, .t0 = b0, .width = a1 - a0, .height = b1 - b0, .scale_x = kx, .scale_y = ky,
                    });
                }
            continue;
        }
        use_tlut(tx, s, 0);
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
 *  kind 6: r3d flat mode on/off (n); kind 7: free the recorded block without running it.
 *  kind 5: pixel (n >> 16, n & 0xFFFF) of the last frame (waits for the RDP): RGBA5551.
 *  kind 0: n shaded z-buffered ~20 px triangles submitted straight from C; returns ticks.
 *  kind 4: same with texture + shade + z (24 px legs, the Rust probe's shape).
 *  kind 1: rspq_block_begin (subsequent rdpq commands are recorded, not executed: measures
 *          CPU-only submission cost); kind 2: rspq_block_end; kind 3: run the recorded
 *          block and wait for RSP+RDP idle (returns ticks), then free it. */
static rspq_block_t *probe_block;
extern bool r3d_probe_recording;
void r3d_set_flat(bool on);

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
        r3d_probe_recording = true;
        return 0;
    case 2:
        probe_block = rspq_block_end();
        r3d_probe_recording = false;
        return 0;
    case 6:
        r3d_set_flat(n != 0);
        return 0;
    case 7:
        if (probe_block) rspq_block_free(probe_block);
        probe_block = NULL;
        return 0;
    case 5: {
        if (!last_fb) return 0;
        rspq_wait();
        uint32_t x = n >> 16, y = n & 0xFFFF;
        if (x >= last_fb->width || y >= last_fb->height) return 0;
        return ((volatile uint16_t *)UncachedAddr(last_fb->buffer))[y * (last_fb->stride / 2) + x];
    }
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
    audio_init(32000, AUDIO_INIT_LATENCY_MS(300));
    mixer_init(MIXER_CHANNELS);
    wav64_init_compression(2); /* ULC (music) */
    wav64_init_compression(3); /* Opus (FMV audio); VADPCM needs no init */
    video_register_codec(&h264_codec);
    sram_init();
    sram_ok = sram_detect() > 0;
    debugf("save: SRAM %s\n", sram_ok ? "found" : "not found (progress is not saved)");
    /* The game's pitch control (SetFrequency) goes up to 100 kHz; Opus is 48 kHz. Allow
     * up to 4x the 32 kHz output (higher pitches are clamped by the Rust side). */
    /* Music / FMV audio (channels 0-1): up to 48 kHz; sound effects (22 kHz sources): 2x. */
    for (int ch = 0; ch < MIXER_CHANNELS; ch++) mixer_ch_set_limits(ch, 16, ch < SFX_CH0 ? 48000 : 44100, 0);

    f2n64_call(f2n64_init);
    for (;;) {
        joypad_poll();
        n64_audio_poll_impl();
        f2n64_call(f2n64_frame);
        n64_audio_poll_impl();
    }
}
