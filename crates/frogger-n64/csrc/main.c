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
/* Frame pacing (app.rs, n64_set_fps_limit): frames shown at most this often (0 = no limit). */
static uint32_t fps_limit;
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
static uint32_t st_mode_ticks, st_modes, st_tex_ticks; /* apply_state statistics */
/* The surface whose whole image (and palette) is in TMEM for rects, or NULL. Rects of a
 * surface that fits one TMEM load upload it once and then cost one texture-rectangle
 * command each, instead of rdpq_tex_blit's generic chunked path (~41 us vs ~3.8 us per
 * rect in ares). Valid within one rect batch only (reset per batch; blits, mode changes
 * and triangles also forget it). */
static const surface_t *rect_tmem;


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

/* ---- RSP / RDP load: a 2 kHz timer samples whether the RSP is running (not halted:
 * rspq halts it when its queue is empty) and whether the RDP pipeline is busy. ---- */
static volatile uint32_t load_samples, rsp_busy_samples, rdp_busy_samples;

static void load_cb(int ovfl) {
    load_samples++;
    if (!(*SP_STATUS & SP_STATUS_HALTED)) rsp_busy_samples++;
    if (*DP_STATUS & DP_STATUS_PIPE_BUSY) rdp_busy_samples++;
}

/* RSP busy permille | RDP busy permille << 16 since the last call (resets). */
uint32_t n64_rcp_load_impl(void) {
    uint32_t n = load_samples ? load_samples : 1;
    uint32_t v = (rsp_busy_samples * 1000 / n) | ((rdp_busy_samples * 1000 / n) << 16);
    load_samples = rsp_busy_samples = rdp_busy_samples = 0;
    return v;
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
/* Phase-3P libdragon variant (tools/n64/README.md): the game's rspq ring is 2 x 32 KiB, but
 * the H.264 decoder recycles its input buffers assuming the stock 2 x 2 KiB (an RSP assert
 * otherwise): limit the window while a video plays. Weak: absent in stock libdragon. */
void rspq_set_lowpri_window(int words) __attribute__((weak));

uint32_t n64_fmv_play_impl(const char *path) {
    if (!file_exists(path)) return 0;
    rspq_wait();
    if (rspq_set_lowpri_window) rspq_set_lowpri_window(0x200);
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
    rspq_wait();
    if (rspq_set_lowpri_window) rspq_set_lowpri_window(0);
    display_set_fps_limit(fps_limit); /* the game's frame pacing */
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

/* ---- large-block pool ----
 * Level loads allocate a few large blocks (texture-bank and mesh files, decode buffers)
 * among thousands of small simulation allocations; after a few level changes the main heap
 * has no contiguous hole for the next level's big files and asset_load/malloc fails (seen
 * on hardware and in ares: frontend -> level -> frontend). Blocks >= BIG_MIN come from this
 * pool, reserved at boot while the heap is clean (first fit, address-ordered, coalescing);
 * the main heap keeps the small blocks. Both fall back to the other on failure. */
#define BIG_MIN (32 * 1024)
#define BIG_POOL (2560 * 1024)
typedef struct big_blk { uint32_t size; uint32_t free; struct big_blk *next; uint32_t pad[13]; } big_blk; /* 64 bytes: payloads 64-byte aligned (surfaces, video buffers) */
void *__real_malloc(size_t);
void *__real_memalign(size_t, size_t);
void *__real_realloc(void *, size_t);
void __real_free(void *);
static uint8_t *big_base;
static big_blk *big_head;
static uint32_t big_used, big_peak;

static void big_init(void) {
    big_base = __real_memalign(64, BIG_POOL);
    if (!big_base) return;
    big_head = (big_blk *)big_base;
    big_head->size = BIG_POOL - sizeof(big_blk);
    big_head->free = 1;
    big_head->next = NULL;
}

static int big_owns(void *p) { return big_base && (uint8_t *)p >= big_base && (uint8_t *)p < big_base + BIG_POOL; }

static void *big_alloc(uint32_t size) {
    size = (size + 63) & ~63u;
    for (big_blk *b = big_head; b; b = b->next) {
        if (!b->free || b->size < size) continue;
        if (b->size >= size + sizeof(big_blk) + 1024) { /* split */
            big_blk *n = (big_blk *)((uint8_t *)(b + 1) + size);
            n->size = b->size - size - sizeof(big_blk);
            n->free = 1;
            n->next = b->next;
            b->next = n;
            b->size = size;
        }
        b->free = 0;
        big_used += b->size;
        if (big_used > big_peak) big_peak = big_used;
        return b + 1;
    }
    return NULL;
}

static void big_free(void *p) {
    big_blk *b = (big_blk *)p - 1;
    b->free = 1;
    big_used -= b->size;
    for (big_blk *c = big_head; c; c = c->next) /* coalesce free neighbours */
        while (c->free && c->next && c->next->free) {
            c->size += sizeof(big_blk) + c->next->size;
            c->next = c->next->next;
        }
}

/* The whole program's malloc family is routed here (`ld --wrap`, tools/n64/Makefile): Rust,
 * our C host and libdragon (asset_load, surfaces, the H.264 decoder's frame buffers, ...). */
void *__wrap_memalign(size_t align, size_t size) {
    if (size >= BIG_MIN && align <= 64) {
        void *p = big_alloc(size);
        if (p) return p;
        debugf("bigpool: %lu bytes do not fit (used %lu KiB of %d), using the main heap\n", (unsigned long)size, (unsigned long)(big_used / 1024), BIG_POOL / 1024);
    }
    void *p = __real_memalign(align < 8 ? 8 : align, size ? size : 1);
    if (!p && size >= 1024 && align <= 64) p = big_alloc(size); /* main heap full: try the pool */
    return p;
}

void *__wrap_malloc(size_t size) { return __wrap_memalign(8, size); }

void *__wrap_calloc(size_t n, size_t size) {
    size_t t = n * size;
    void *p = __wrap_memalign(8, t);
    if (p) memset(p, 0, t);
    return p;
}

void __wrap_free(void *ptr) {
    if (!ptr) return;
    if (big_owns(ptr)) big_free(ptr);
    else __real_free(ptr);
}

void *__wrap_realloc(void *ptr, size_t size) {
    if (!ptr) return __wrap_malloc(size);
    if (big_owns(ptr)) {
        big_blk *b = (big_blk *)ptr - 1;
        if (size <= b->size) return ptr;
        void *n = __wrap_malloc(size);
        if (n) { memcpy(n, ptr, b->size); big_free(ptr); }
        return n;
    }
    void *r = __real_realloc(ptr, size);
    if (!r && size >= 1024) {
        void *n = big_alloc(size);
        if (n) { memcpy(n, ptr, malloc_usable_size(ptr)); __real_free(ptr); }
        return n;
    }
    return r;
}

void *n64_alloc_impl(uint32_t size, uint32_t align) { return memalign(align < 8 ? 8 : align, size ? size : 1); }

void n64_free_impl(void *ptr) { free(ptr); }

uint32_t n64_ticks_impl(void) { return TICKS_READ(); }

uint32_t n64_memory_size_impl(void) { return get_memory_size(); }

void n64_heap_stats_impl(n64_heap_t *out) {
    heap_stats_t s;
    sys_get_heap_stats(&s);
    out->total = s.total;
    /* The pool is one reserved block of the main heap: count what is used inside it. */
    out->used = big_base ? s.used - BIG_POOL + big_used : s.used;
}

uint32_t n64_pad_impl(uint32_t port) {
    if (port > 3) return 0;
    joypad_buttons_t b = joypad_get_buttons(port);
    joypad_inputs_t in = joypad_get_inputs(port);
    return (uint32_t)b.raw | ((uint32_t)(uint8_t)in.stick_x << 16) | ((uint32_t)(uint8_t)in.stick_y << 24);
}

/* Whole file (mkasset-decompressed) into a buffer from n64_alloc_impl, so large files land
 * in the large-block pool (asset_load would malloc them from the main heap). */
void *n64_file_read_impl(const char *path, uint32_t *len) {
    int sz = 0;
    if (!file_exists(path)) return NULL;
    FILE *f = asset_fopen(path, &sz);
    if (!f) return NULL;
    uint8_t *p = memalign(16, sz > 0 ? (size_t)sz : 1);
    assertf(p, "Out of memory reading %s (%d bytes; pool %lu/%d KiB)", path, sz, (unsigned long)(big_used / 1024), BIG_POOL / 1024);
    int got = 0;
    while (got < sz) {
        int n = fread(p + got, 1, sz - got, f);
        if (n <= 0) break;
        got += n;
    }
    fclose(f);
    if (len) *len = (uint32_t)got;
    return p;
}

uint32_t n64_bigpool_peak_kib(void) { return big_peak / 1024; }

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
    rect_tmem = NULL;
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
    tex_format_t sf = surface_get_format(s);
    if (sx == 1 && sy == 1 && (sf == FMT_RGBA32 || sf == FMT_RGBA16)) {
        /* Fast path (procedural textures are refilled every frame, phase 3P): one texel per
         * output texel, the same integer maths as below with the divisions by the
         * brightest channel done by reciprocal (exact: numerators < 2^16, divisors < 256). */
        static uint32_t recip[256];
        if (!recip[1])
            for (int m = 1; m < 256; m++) recip[m] = (uint32_t)(0xFFFFFFFFu / m) + 1;
        for (int y = 0; y < nh; y++) {
            const uint8_t *row = (const uint8_t *)s->buffer + y * s->stride;
            /* surface_alloc memory is uncached: one 32-bit store per texel. */
            uint32_t *dst = (uint32_t *)((uint8_t *)t->addv.buffer + y * t->addv.stride);
            for (int x = 0; x < nw; x++) {
                uint32_t r, g, b, a;
                if (sf == FMT_RGBA32) {
                    r = row[x * 4]; g = row[x * 4 + 1]; b = row[x * 4 + 2]; a = row[x * 4 + 3];
                } else {
                    uint32_t v = (row[x * 2] << 8) | row[x * 2 + 1];
                    r = ((v >> 11) & 31) * 255 / 31; g = ((v >> 6) & 31) * 255 / 31; b = ((v >> 1) & 31) * 255 / 31;
                    a = (v & 1) ? 255 : 0;
                }
                r = r * a / 255; g = g * a / 255; b = b * a / 255;
                uint32_t m = r > g ? (r > b ? r : b) : (g > b ? g : b);
                uint32_t o = m;
                if (m) {
                    uint64_t k = recip[m];
                    o |= (uint32_t)(((uint64_t)(r * 255) * k) >> 32) << 24;
                    o |= (uint32_t)(((uint64_t)(g * 255) * k) >> 32) << 16;
                    o |= (uint32_t)(((uint64_t)(b * 255) * k) >> 32) << 8;
                }
                dst[x] = o;
            }
        }
        t->has_addv = true;
        return;
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
    uint32_t mt0 = TICKS_READ();
    if (cur_kind != kind || st->flags != cur_state.flags || st->blend != cur_state.blend ||
        st->alpha_ref != cur_state.alpha_ref || (st->tex == 0) != (cur_state.tex == 0)) {
        st_modes++;
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
        rect_tmem = NULL;
    }
    if (kind == 0 && tex && (st->tex != cur_tex_loaded || ((st->flags ^ cur_state.flags) & N64_WRAP))) {
        rdpq_texparms_t parms = {0};
        if (st->flags & N64_WRAP) parms.s.repeats = parms.t.repeats = REPEAT_INFINITE;
        const surface_t *s = tri_surface(st);
        use_tlut(&textures[st->tex], s, 0);
        uint32_t tt0 = TICKS_READ();
        rdpq_tex_upload(TILE0, s, &parms);
        st_tex_ticks += TICKS_READ() - tt0;
        cur_tex_loaded = st->tex;
    }
    if (kind == 1) cur_tex_loaded = 0; /* blits reload TMEM */
    cur_state = *st;
    cur_kind = kind;
    st_mode_ticks += TICKS_READ() - mt0;
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

/* CPU-triangle statistics (logged every 300 presented frames). */
extern volatile uint32_t rspq_wait_ticks;
static uint32_t st_tri_ticks, st_rect_ticks, st_rects, st_tri_wait, st_rect_wait, st_wait0;
static uint32_t st_calls, st_src_tris, st_rdpq_tris, st_tiled_src, st_tiled_uploads, st_frames;

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
            st_tiled_uploads++;
            st_rdpq_tris += n - 2;
            for (int k = 1; k + 1 < n; k++)
                rdpq_triangle(f, (const float *)&a[0], (const float *)&a[k], (const float *)&a[k + 1]);
        }
    }
    cur_tex_loaded = 0;
}

static void draw_tris_inner(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st);
void n64_draw_tris_impl(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    uint32_t t0 = TICKS_READ(), w0 = rspq_wait_ticks;
    draw_tris_inner(v, ntris, st);
    st_tri_ticks += TICKS_READ() - t0;
    st_tri_wait += rspq_wait_ticks - w0;
}
static void draw_tris_inner(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    if (!attached || !ntris) return;
    st_calls++;
    st_src_tris += ntris;
    rect_tmem = NULL;
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
        st_tiled_src += ntris;
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
        st_rdpq_tris += ntris;
        return;
    }
    st_rdpq_tris += ntris;
    for (uint32_t k = 0; k < ntris; k++, v += 3)
        rdpq_triangle(f, (const float *)&v[0], (const float *)&v[1], (const float *)&v[2]);
}

static void draw_rects_inner(const n64_rect_t *r, uint32_t n, const n64_state_t *st);
void n64_draw_rects_impl(const n64_rect_t *r, uint32_t n, const n64_state_t *st) {
    uint32_t t0 = TICKS_READ(), w0 = rspq_wait_ticks;
    st_rects += n;
    draw_rects_inner(r, n, st);
    st_rect_ticks += TICKS_READ() - t0;
    st_rect_wait += rspq_wait_ticks - w0;
}

/* Draws [x0,x1)x[y0,y1) from texels s0..s1 / t0..t1 of `s` (s1 < s0 / t1 < t0 = flipped)
 * when `s` fits TMEM; false = use the blit path. */
static bool rect_fast(const tex_t *tx, const surface_t *s, float x0, float y0, float x1, float y1, float s0, float t0, float s1, float t1) {
    if (!fits_tmem_block(s)) return false;
    if (rect_tmem != s) {
        use_tlut(tx, s, 0);
        rdpq_tex_upload(TILE0, s, NULL);
        rect_tmem = s;
    }
    rdpq_texture_rectangle_scaled(TILE0, x0, y0, x1, y1, s0, t0, s1, t1);
    return true;
}

/* A sub-rectangle of a surface too big for TMEM (e.g. a glyph of the font atlas): uploads
 * just that part (+1 texel border for bilinear, clamped to the image) with
 * rdpq_tex_upload_sub, which keeps the full image's texel coordinates, and draws one
 * texture rectangle. false = it does not fit TMEM (use the blit path). */
static bool rect_sub(const tex_t *tx, const surface_t *s, const n64_rect_t *r, int is0, int it0, int is1, int it1, bool fx, bool fy, bool filter) {
    tex_format_t f = surface_get_format(s);
    if (f == FMT_CI4 || f == FMT_CI8 || TEX_FORMAT_BITDEPTH(f) > 16) return false;
    int b = filter ? 1 : 0;
    int u0 = is0 - b < 0 ? 0 : is0 - b, v0 = it0 - b < 0 ? 0 : it0 - b;
    int u1 = is1 + b > s->width ? s->width : is1 + b, v1 = it1 + b > s->height ? s->height : it1 + b;
    int row = (TEX_FORMAT_PIX2BYTES(f, u1 - u0) + 7) & ~7;
    if (row * (v1 - v0) > 4096) return false;
    use_tlut(tx, s, 0); /* switches palette lookup off (a CI rect may have turned it on) */
    rdpq_tex_upload_sub(TILE0, s, NULL, u0, v0, u1, v1);
    rdpq_texture_rectangle_scaled(TILE0, r->x0, r->y0, r->x1, r->y1, fx ? is1 : is0, fy ? it1 : it0, fx ? is0 : is1, fy ? it0 : it1);
    return true;
}

/* One rect of draw_rects_inner; returns the path taken (statistics). */
static int draw_one_rect(const n64_rect_t *r, const n64_state_t *st, bool tex, const tex_t *tx) {
    rdpq_set_prim_color(color_from_packed32(r->rgba));
    if (!tex) {
        rdpq_fill_rectangle(r->x0, r->y0, r->x1, r->y1);
        return 1;
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
    if (sw <= 0 || sh <= 0 || r->x1 <= r->x0 || r->y1 <= r->y0) return 2;
    if (tx->per_tile && tx->tile_w > 0 && tx->tile_h > 0 && !fx && !fy) {
        rect_tmem = NULL;
    /* One palette per tile: blit tile by tile (unfiltered, as the backdrops are). */
        float kx = (r->x1 - r->x0) / sw, ky = (r->y1 - r->y0) / sh;
        int ntx = (s->width + tx->tile_w - 1) / tx->tile_w;
        for (int ty = it0 / tx->tile_h * tx->tile_h; ty < it1; ty += tx->tile_h)
            for (int tx0 = is0 / tx->tile_w * tx->tile_w; tx0 < is1; tx0 += tx->tile_w) {
                int a0 = tx0 > is0 ? tx0 : is0, b0 = ty > it0 ? ty : it0;
                int a1 = tx0 + tx->tile_w < is1 ? tx0 + tx->tile_w : is1, b1 = ty + tx->tile_h < it1 ? ty + tx->tile_h : it1;
                if (a1 <= a0 || b1 <= b0) return 3;
                use_tlut(tx, s, (ty / tx->tile_h) * ntx + tx0 / tx->tile_w);
                rdpq_tex_blit(s, r->x0 + (a0 - is0) * kx, r->y0 + (b0 - it0) * ky, &(rdpq_blitparms_t){
                    .s0 = a0, .t0 = b0, .width = a1 - a0, .height = b1 - b0, .scale_x = kx, .scale_y = ky,
                });
            }
        return 4;
    }
    /* Minified blits of textures larger than TMEM (HUD clock 128x128 drawn ~30 px wide):
     * the TMEM-sized box-filtered copy has at least one texel per pixel, so blit it
     * instead (one texture load instead of one per TMEM-sized piece; phase 3P). */
    if (tx->nfull == 1 && !tx->per_tile && !fits_tmem_block(s)) {
        const surface_t *tm = n64_tex_tmem_surface(st->tex);
        if (tm && tm != s && tm->width > 0 && tm->height > 0) {
            float kx = (float)tm->width / s->width, ky = (float)tm->height / s->height;
            int a0 = (int)(is0 * kx), b0 = (int)(it0 * ky), a1 = (int)(is1 * kx + 0.5f), b1 = (int)(it1 * ky + 0.5f);
            if (a1 > a0 && b1 > b0 && r->x1 - r->x0 <= a1 - a0 && r->y1 - r->y0 <= b1 - b0) {
                if (rect_fast(tx, tm, r->x0, r->y0, r->x1, r->y1, fx ? a1 : a0, fy ? b1 : b0, fx ? a0 : a1, fy ? b0 : b1)) return 6;
                rect_tmem = NULL;
                use_tlut(tx, tm, 0);
                rdpq_tex_blit(tm, r->x0, r->y0, &(rdpq_blitparms_t){
                    .s0 = a0, .t0 = b0, .width = a1 - a0, .height = b1 - b0, .flip_x = fx, .flip_y = fy,
                    .scale_x = (r->x1 - r->x0) / (a1 - a0), .scale_y = (r->y1 - r->y0) / (b1 - b0),
                    .filtering = (st->flags & N64_BILINEAR) != 0,
                });
                return 5;
            }
        }
    }
    if (tx->nfull == 1 && rect_fast(tx, s, r->x0, r->y0, r->x1, r->y1, fx ? is1 : is0, fy ? it1 : it0, fx ? is0 : is1, fy ? it0 : it1)) return 7;
    rect_tmem = NULL;
    if (tx->nfull == 1 && rect_sub(tx, s, r, is0, it0, is1, it1, fx, fy, (st->flags & N64_BILINEAR) != 0)) return 8;
    use_tlut(tx, s, 0);
    rdpq_tex_blit(s, r->x0, r->y0, &(rdpq_blitparms_t){
        .s0 = is0, .t0 = it0, .width = sw, .height = sh, .flip_x = fx, .flip_y = fy,
        .scale_x = (r->x1 - r->x0) / sw, .scale_y = (r->y1 - r->y0) / sh,
        .filtering = (st->flags & N64_BILINEAR) != 0,
    });
    return 0;
}

static void draw_rects_inner(const n64_rect_t *r, uint32_t n, const n64_state_t *st) {
    if (!attached || !n) return;
    apply_state(st, 1);
    /* Only within one batch: between batches the RSP renderer's lists load TMEM too. */
    rect_tmem = NULL;
    bool tex = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used;
    const tex_t *tx = tex ? &textures[st->tex] : NULL;
    for (uint32_t k = 0; k < n; k++, r++) {
        draw_one_rect(r, st, tex, tx);
    }
}


void n64_present_impl(void) {
    if (++st_frames == 300) {
        debugf("cputri: per frame %lu calls, %lu source tris (%lu tiled), %lu rdpq triangles, %lu tile uploads, tris %lu us (wait %lu); %lu rects %lu us (wait %lu); all rspq waits %lu us; apply_state %lu us (%lu mode resets, tri tex uploads %lu us)\n", st_calls / 300, st_src_tris / 300, st_tiled_src / 300, st_rdpq_tris / 300, st_tiled_uploads / 300, TICKS_TO_US(st_tri_ticks) / 300, TICKS_TO_US(st_tri_wait) / 300, st_rects / 300, TICKS_TO_US(st_rect_ticks) / 300, TICKS_TO_US(st_rect_wait) / 300, TICKS_TO_US(rspq_wait_ticks - st_wait0) / 300, TICKS_TO_US(st_mode_ticks) / 300, st_modes / 300, TICKS_TO_US(st_tex_ticks) / 300);
        st_mode_ticks = st_modes = st_tex_ticks = 0;
        st_tri_wait = st_rect_wait = 0; st_wait0 = rspq_wait_ticks;
        st_tri_ticks = st_rect_ticks = st_rects = 0;
        st_frames = st_calls = st_src_tris = st_rdpq_tris = st_tiled_src = st_tiled_uploads = 0;
    }
    if (!attached) return;
    rdpq_detach_show();
    attached = false;
}

void n64_rdp_wait_impl(void) { rspq_wait(); }

void n64_set_fps_limit_impl(uint32_t fps) {
    fps_limit = fps;
    display_set_fps_limit(fps);
}

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

/* Every non-empty bucket (address order; prof.py aggregates them per function). Buckets with
 * a single sample are skipped once more than 1500 lines were printed. */
static void prof_top(const char *what, uint16_t *h) {
    int lines = 0;
    for (uint32_t i = 0; i < PROF_BUCKETS; i++) {
        if (!h[i] || (h[i] < 2 && lines > 1500)) continue;
        debugf("PROF %s %08lx %u\n", what, (unsigned long)(PROF_BASE + (i << PROF_SHIFT)), h[i]);
        lines++;
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

static void floor_selftest(void);

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

    timer_init();
    new_timer(TIMER_TICKS(TICKS_PER_SECOND / 2000), TF_CONTINUOUS, load_cb);

    /* After the boot-time allocations (framebuffers, z-buffer, audio, RSP queues), which live
     * for the whole run, so the pool holds only per-level and transient large blocks. */
    big_init();
    floor_selftest();
    debugf("large-block pool %d KiB\n", big_base ? BIG_POOL / 1024 : 0);
    f2n64_call(f2n64_init);
    for (;;) {
        joypad_poll();
        n64_audio_poll_impl();
        f2n64_call(f2n64_frame);
        n64_audio_poll_impl();
    }
}

/* libdragon builds with -ffast-math (no signed zeros, no NaNs): the floor code must not be. */
#define F2_NO_FAST_MATH __attribute__((optimize("no-fast-math")))
#define F2_EXACT_FP __attribute__((optimize("no-fast-math"), noinline))

/* ---- floor / floorf on the FPU --------------------------------------------------------
 * compiler_builtins' generic floor/floorf (weak symbols) run ~100+ integer instructions;
 * libdragon's CPU triangle setup calls floor() ~10x and floorf() 6x per triangle
 * (float_to_s16_16, edge Y snapping) and the Rust code calls them too. The VR4300 rounds
 * toward -inf in one instruction (floor.w.s / floor.w.d) for results that fit 32 bits; the
 * rest (|x| >= 2^31, inf, NaN) are integral for float and take musl's exact bit-level
 * floor (on 32-bit halves) for double. floor is exact, so the
 * results are bit-identical (including -0.0, which the integer path would lose). */
F2_NO_FAST_MATH static float floorf_bits(float x) {
    union { float f; uint32_t i; } u = { x };
    int e = (int)(u.i >> 23 & 0xff) - 0x7f;
    uint32_t m;
    if (e >= 23) return x;
    if (e >= 0) {
        m = 0x007fffff >> e;
        if ((u.i & m) == 0) return x;
        if (u.i >> 31) u.i += m;
        u.i &= ~m;
    } else {
        if (u.i >> 31 == 0) u.i = 0;
        else if (u.i << 1) u.f = -1.0f;
    }
    return u.f;
}

/* musl's floor on the two 32-bit halves of the double (w[FW_HI] = sign, exponent,
 * high mantissa); no 64-bit integer operations (GCC's handling of a double in a 64-bit
 * integer went wrong here: floor_bits-style code gave -0.0 for 0.9 on the N64). */
#if __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define FW_HI 0
#define FW_LO 1
#else
#define FW_HI 1
#define FW_LO 0
#endif
F2_NO_FAST_MATH static double floor_words(double x) {
    uint32_t w[2];
    memcpy(w, &x, 8);
    int e = (int)((w[FW_HI] >> 20) & 0x7ff) - 0x3ff;
    if (e >= 52) return x;  /* integral, inf or NaN */
    if (e < 0) {            /* |x| < 1 */
        if ((w[FW_HI] >> 31) == 0) return 0.0;
        if ((w[FW_HI] << 1) | w[FW_LO]) return -1.0;
        return x;           /* -0.0 */
    }
    int fb = 52 - e;        /* fractional bits, 1..52 */
    uint32_t mlo = fb >= 32 ? 0xffffffffu : (1u << fb) - 1;
    uint32_t mhi = fb >= 32 ? (1u << (fb - 32)) - 1 : 0;
    if (((w[FW_HI] & mhi) | (w[FW_LO] & mlo)) == 0) return x;
    if (w[FW_HI] >> 31) {
        uint32_t lo = w[FW_LO] + mlo;
        w[FW_HI] += mhi + (lo < w[FW_LO]);
        w[FW_LO] = lo;
    }
    w[FW_HI] &= ~mhi;
    w[FW_LO] &= ~mlo;
    memcpy(&x, w, 8);
    return x;
}

F2_EXACT_FP float floorf(float x) {
    if (__builtin_fabsf(x) < 2147483648.0f) {
        int32_t r;
        float t;
        __asm__("floor.w.s %1, %2\n\tmfc1 %0, %1" : "=r"(r), "=&f"(t) : "f"(x));
        if (r == 0) return x == 0.0f ? x : 0.0f;
        return (float)r;
    }
    return x; /* |x| >= 2^31 (always integral for float), inf or NaN */
}

F2_EXACT_FP double floor(double x) {
    if (__builtin_fabs(x) < 2147483648.0) {
        int32_t r;
        float t;
        __asm__("floor.w.d %1, %2\n\tmfc1 %0, %1" : "=r"(r), "=&f"(t) : "f"(x));
        if (r == 0) return x == 0.0 ? x : 0.0;
        return (double)r;
    }
    return floor_words(x);
}

/* Boot check of the FPU floor paths against the exact bit-level ones (debug log). */
F2_NO_FAST_MATH static void floor_selftest(void) {
    uint32_t r = 0x12345678u;
    int bad = 0;
    for (int k = 0; k < 20000; k++) {
        r ^= r << 13; r ^= r >> 17; r ^= r << 5;
        float f = (float)(int32_t)r / (float)(1u << (r % 24));
        float (*volatile ff)(float) = floorf;
        double (*volatile fd)(double) = floor;
        float a = ff(f), b = floorf_bits(f);
        double d = (double)(int32_t)r / (double)(1u << (r % 30)) - 0.5;
        double c = fd(d), e = floor_words(d);
        if (memcmp(&a, &b, 4) || memcmp(&c, &e, 8)) {
            if (bad < 6) {
                uint32_t fi, ai, bi; uint64_t di, ci, ei;
                memcpy(&fi, &f, 4); memcpy(&ai, &a, 4); memcpy(&bi, &b, 4);
                memcpy(&di, &d, 8); memcpy(&ci, &c, 8); memcpy(&ei, &e, 8);
                debugf("floor mismatch f=%08lx hw=%08lx ref=%08lx | d=%016llx hw=%016llx ref=%016llx\n", fi, ai, bi, di, ci, ei);
            }
            bad++;
        }
    }
    float (*volatile fz)(float) = floorf;
    float z = fz(-0.0f);
    if (!__builtin_signbit(z)) bad++;
    debugf("floor selftest: %d mismatches\n", bad);
}

void n64_cache_writeback_impl(const void *p, uint32_t len) {
    if (len) data_cache_hit_writeback(p, len);
}
