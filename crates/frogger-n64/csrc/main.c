/* libdragon host of the Frogger 2 N64 port: boot, display, RDP, audio, joypad, DFS.
 * Rust (libfrogger_n64.a) owns the frame: main() calls f2n64_init() once and f2n64_frame()
 * per frame through the f2n64_call bridge; Rust calls back the n64_* functions below
 * (via the abi.S stubs). Interface and ABI rules: n64_ffi.h. */
#include <libdragon.h>
#include <malloc.h>
#include <backtrace.h>
#include <string.h>
#include "n64_ffi.h"

#define MAX_TEXTURES 2048
#define MAX_SFX 1024
#define MIXER_CHANNELS 24
#define MUSIC_CH 0 /* stereo: channels 0 and 1 */
#define SFX_CH0 2

static surface_t zbuf;
void n64_set_fps_limit_impl(uint32_t fps);
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
    bool tiled;         /* N64_NTX_TILED: triangles use the TMEM tiles even with a small copy */
    surface_t tmem;     /* r3d: downscaled copy of a texture larger than one TMEM load */
    bool has_tmem;
} tex_t;
static int cur_tlut_mode = -1; /* TLUT mode currently set in the SOM (-1 = unknown) */
static void make_add_variant(tex_t *t);
static void slow_batch(const char *what, uint32_t n, const n64_state_t *st, uint32_t ticks, uint32_t wait, const uint32_t *paths0);
static void make_add_variant_from(tex_t *t, const void *src, uint32_t src_stride);
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

static void big_dump(void);
uint32_t n64_mem_phase_impl(uint32_t phase);
uint32_t mem_phase_save_and_play(void);
void mem_phase_restore(uint32_t o);
void mem_main_begin(void);
void mem_main_end(void);
void n64_mem_report_impl(const char *tag, uint32_t len, uint32_t check);

static uint32_t sfx_open(const char *path, uint32_t looped);

/* Main heap: newlib keeps the FILE structures of a level's many open sample streams for the
 * whole run (and the wav64 headers are small). */
uint32_t n64_sfx_open_impl(const char *path, uint32_t looped) {
    mem_main_begin();
    uint32_t h = sfx_open(path, looped);
    mem_main_end();
    return h;
}

static uint32_t sfx_open(const char *path, uint32_t looped) {
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
    /* The mixer allocates a channel's sample buffer at its first play and keeps it: session
     * memory, even when that first play is during a level load. */
    mem_main_begin();
    mixer_ch_play(ch, &sfx[h]->wave);
    mem_main_end();
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
#ifdef F2N64_NO_MUSIC
    return 0;
#endif
    if (!path || !file_exists(path)) return 0;
    /* The track plays on through level loads: main heap (and the mixer's channel buffers,
     * allocated at the first play, stay for the whole run). */
    mem_main_begin();
    music = wav64_load(path, &(wav64_loadparms_t){.streaming_mode = WAV64_STREAMING_FULL});
    if (music) {
        wav64_set_loop(music, looped != 0);
        wav64_play(music, MUSIC_CH);
    }
    mem_main_end();
    return music != NULL;
}

void n64_music_vol_impl(uint32_t vol_permille) {
    float v = vol_permille / 1000.0f;
    mixer_ch_set_vol(MUSIC_CH, v, v);
}

/* Longest time between two polls (C0 ticks) since n64_audio_gap_take: the music buffer
 * holds 300 ms, longer gaps are audible dropouts. */
uint32_t audio_last_poll; /* read by Rust (rt::audio_poll_due) */
static uint32_t audio_max_gap;

static uint32_t audio_polls, audio_ticks, audio_t0;
/* C0 ticks spent in mixer polls since boot (wraps): read by Rust (rt::audio_ticks_total) for
 * the per-frame audio cost in the perf and spike logs. */
uint32_t audio_ticks_total;
void n64_audio_poll_impl(void) {
    uint32_t now = TICKS_READ();
    if (audio_last_poll && now - audio_last_poll > audio_max_gap) audio_max_gap = now - audio_last_poll;
    audio_last_poll = now;
    if (!audio_t0) audio_t0 = now;
    audio_polls++;
#ifndef F2N64_NO_MIXER /* testing: no mixer rounds at all (no RSP audio work) */
    if (audio_can_write()) mixer_try_play();
#endif
    uint32_t spent = TICKS_READ() - now;
    audio_ticks += spent;
    audio_ticks_total += spent;
    if (now - audio_t0 > TICKS_FROM_MS(5000)) {
        /* CPU time of the mixer (incl. music decoding) per second of real time. */
        debugf("audio: %lu polls/s, %lu us/s in polls\n", audio_polls * 1000 / TICKS_TO_MS(now - audio_t0), TICKS_TO_US(audio_ticks) * 1000 / TICKS_TO_MS(now - audio_t0));
        audio_polls = audio_ticks = 0;
        audio_t0 = now;
    }
}

/* n64_audio_poll_impl if 40 ms have passed since the last poll: cheap enough for loops that
 * can run long once (a level's first frame uploads all its textures). */
void n64_audio_poll_due_impl(void) {
    if (TICKS_READ() - audio_last_poll > TICKS_FROM_MS(40)) n64_audio_poll_impl();
}

uint32_t n64_audio_gap_take_impl(void) {
    uint32_t g = audio_max_gap;
    audio_max_gap = 0;
    return g;
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

/* Set by test boots (Rust `rt::set_fmv_autoskip`): every video stops after its first frames. */
volatile int32_t n64_fmv_autoskip;

static void fmv_osd(void *ctx, int frame_idx, float time_sec, fmv_control_t *ctrl) {
    /* Skippable with A / B / Start like the PC's key press. */
    joypad_poll();
    joypad_buttons_t b = joypad_get_buttons_pressed(JOYPAD_PORT_1);
    if (frame_idx > 2 && (b.a || b.b || b.start)) ctrl->stop(ctrl);
    if (n64_fmv_autoskip && frame_idx > 2) ctrl->stop(ctrl); /* testing: as if skipped at once */
}

/* Plays a video over the game's 320x240 16-bit display (no display re-init: the game's
 * framebuffers stay); returns 0 if the file is missing. */
/* Phase-3P libdragon variant (tools/n64/README.md): the game's rspq ring is 2 x 32 KiB, but
 * the H.264 decoder recycles its input buffers assuming the stock 2 x 2 KiB (an RSP assert
 * otherwise): limit the window while a video plays. Weak: absent in stock libdragon. */
void rspq_set_lowpri_window(int words) __attribute__((weak));

uint32_t n64_fmv_play_impl(const char *path) {
    if (!file_exists(path)) return 0;
    n64_mem_report_impl("video", 5, 0);
#ifdef F2N64_BIGPOOL_DUMP
    big_dump();
#endif
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
    /* The decoder's buffers are transient: the main heap / the level heap's top (a story FMV
     * plays in the middle of a level load). */
    uint32_t ph = mem_phase_save_and_play();
    fmv_play(path, &p);
    mem_phase_restore(ph);
    rspq_wait();
    if (rspq_set_lowpri_window) rspq_set_lowpri_window(0);
    n64_set_fps_limit_impl(fps_limit); /* the game's frame pacing */
    mixer_ch_set_limits(FMV_CH, 16, 48000, 0);
    mixer_ch_set_limits(FMV_CH + 1, 16, 48000, 0);
    cur_kind = -1;
    cur_tex_loaded = 0;
    cur_tlut_mode = -1;
    return 1;
}

/* ---- saves: cartridge SRAM (256 Kbit) ---- */
static bool sram_ok;

uint32_t n64_save_read_impl(void *dst, uint32_t off, uint32_t len) {
    if (!sram_ok || off > 0x8000 || len > 0x8000 - off) return 0;
    return sram_read(dst, off, len) == (int)len ? len : 0;
}

uint32_t n64_save_write_impl(const void *src, uint32_t off, uint32_t len) {
    if (!sram_ok || off > 0x8000 || len > 0x8000 - off) return 0;
    return sram_write(src, off, len) == (int)len ? len : 0;
}

/* ---- misc ---- */
void n64_log_impl(const char *msg, uint32_t len) { debugf("%.*s", (int)len, msg); }

void n64_panic_impl(const char *msg, uint32_t len) {
    debugf("RUST PANIC: %.*s\n", (int)len, msg);
    assertf(0, "Rust panic: %.*s", (int)len, msg);
    abort();
}

/* ---- memory: the level heap ----
 * Two heaps by lifetime, so every level change starts from an empty one:
 * - the main heap (newlib malloc): the boot, the session (MEM_MAIN: fonts, the sfx index, the
 *   music stream) and small play-time blocks (the game's churn);
 * - the level heap (this pool, the rest of RDRAM after boot): everything allocated while a
 *   level loads (any size: the simulation's InitLevel from the bottom, MEM_LEVEL; the
 *   renderer's `App::load_scene` from the top, MEM_LEVEL_TOP) and large play-time blocks
 *   (from the top: the renderer's per-level storage growing in the first frames, the story
 *   book page, video buffers). The renderer frees its last tables (image registry, sound
 *   tables) only once `load_scene` starts, after InitLevel allocated the new level: kept at
 *   the other end, they do not cut holes into it. The next level change frees all of it (the renderer side in
 *   `unload_level_render`, the simulation's level state in InitLevel), so the next load
 *   finds the pool empty: no fragmentation carries over from one level to the next, whatever
 *   the order of the frees. (Before: one 2.5 MiB pool for blocks >= 32 KiB whatever their
 *   lifetime, small blocks spilling into it when the main heap was full: loads failed on
 *   fragmentation, e.g. 1.2 MiB free and no 439 KiB hole for titles.ntx.)
 * Each heap falls back to the other when full. Every block records the load (epoch) it was
 * allocated in; `n64_mem_report` (the `mem` log lines) counts blocks of earlier loads still
 * alive, which must stay ~0 (anything that outlives its level belongs in MEM_MAIN, Rust
 * `rt::mem_play`). Budget and measurements: research/n64-memory.md "Level heap".
 * Allocator: boundary tags (16-byte headers, 16-byte granularity), one address-ordered free
 * list, first fit from the bottom or from the top, O(1) coalescing. */
typedef struct pb {
    uint32_t size; /* whole block incl. this header, multiple of 16; bit 0: in use */
    uint32_t prev; /* size of the block below (0: the first) */
    union { struct pb *nf; uint32_t epoch; }; /* free: list links; used: load epoch (0: play) */
    union { struct pb *pf; uint32_t req; };   /* used: requested bytes */
} pb;
_Static_assert(sizeof(pb) == 16, "pool header");
#define PB_USED 1u
#define PB_SZ(b) ((b)->size & ~15u)
#define PB_MIN 32u /* smallest block: header + 16 */
/* What the main heap keeps after boot for the session and play; the rest is the level heap.
 * research/n64-memory.md "Level heap" has the measurements behind it. */
#ifndef F2N64_MAIN_RESERVE_KIB
#define F2N64_MAIN_RESERVE_KIB 1500
#endif
void *__real_malloc(size_t);
void *__real_memalign(size_t, size_t);
void *__real_realloc(void *, size_t);
void __real_free(void *);
static uint8_t *pool_base, *pool_end;
static uint32_t pool_size;
static pb *pool_free_list;
static uint32_t pool_used, pool_peak, pool_runs;
/* 0: play (main heap first), 1: level load (level heap first). */
static uint32_t mem_phase, mem_epoch = 1;
static uint32_t mem_fallback_play, mem_fallback_level, mem_fallback_level_bytes;
static uint32_t mem_main_only, mem_main_save, mem_level_top;
#define PLAY_BIG (16 * 1024)
#ifdef F2N64_MEM_TRACE_SIZE /* testing (tools/n64/README.md) */
#ifndef F2N64_MEM_TRACE_SIZE2
#define F2N64_MEM_TRACE_SIZE2 F2N64_MEM_TRACE_SIZE
#endif
#ifndef F2N64_MEM_TRACE_PLAY
#define F2N64_MEM_TRACE_PLAY 0xffffffffu
#endif
#endif
static int mem_main_peak, mem_main_used; /* main heap bytes in blocks allocated since big_init */

/* Main-heap accounting (blocks allocated after big_init; the boot's own are in the base). */
static inline void main_count(void *p, int sign) {
    if (!pool_base) return;
    mem_main_used += sign * (int)malloc_usable_size(p);
    if (mem_main_used > mem_main_peak) mem_main_peak = mem_main_used;
}

static inline pb *pb_next(pb *b) { return (pb *)((uint8_t *)b + PB_SZ(b)); }
/* The free list is kept in address order: first fit stops early, and the top search walks
 * it backwards. `fl_hint` (a member, or NULL) is where the last insertion happened: frees
 * come in runs of nearby addresses, so the sorted insertion walks only a few steps. */
static pb *pool_free_tail, *fl_hint;
static inline void fl_link_after(pb *h, pb *b) { /* h == NULL: at the head */
    b->pf = h;
    b->nf = h ? h->nf : pool_free_list;
    if (b->nf) b->nf->pf = b;
    else pool_free_tail = b;
    if (h) h->nf = b;
    else pool_free_list = b;
    pool_runs++;
}
static void fl_insert(pb *b) {
    pb *h = fl_hint ? fl_hint : pool_free_list;
    if (!h) {
        fl_link_after(NULL, b);
    } else if (h < b) {
        while (h->nf && h->nf < b) h = h->nf;
        fl_link_after(h, b);
    } else {
        while (h->pf && h->pf > b) h = h->pf;
        fl_link_after(h->pf, b);
    }
    fl_hint = b;
}
static inline void fl_unlink(pb *b) {
    if (fl_hint == b) fl_hint = b->nf ? b->nf : b->pf;
    if (b->pf) b->pf->nf = b->nf;
    else pool_free_list = b->nf;
    if (b->nf) b->nf->pf = b->pf;
    else pool_free_tail = b->pf;
    pool_runs--;
}
/* `n` takes the list place of `o` (no free block lies between them). */
static inline void fl_replace(pb *o, pb *n) {
    n->nf = o->nf;
    n->pf = o->pf;
    if (n->pf) n->pf->nf = n;
    else pool_free_list = n;
    if (n->nf) n->nf->pf = n;
    else pool_free_tail = n;
    if (fl_hint == o) fl_hint = n;
}

/* Stack guard: libdragon's stack is the top 64 KiB of RDRAM, directly above the heap, and
 * nothing stops it from growing into the heap. `App::new` builds the App on the stack and
 * goes ~135 KiB below it (measured); with the heap reaching the top (a full main heap, or a
 * pool up to there) that overwrote live blocks. A painted guard below the stack (taken from
 * the heap's top) covers the boot; once the App is up (`rt::mem_report_stack_restart`) all
 * but STACK_GUARD_KEEP of it goes back to the heap, and the rest stays painted: `mem` lines
 * show how far the stack went into it (must stay 0). */
#ifndef F2N64_STACK_GUARD_KIB
#define F2N64_STACK_GUARD_KIB 192
#endif
#define STACK_GUARD (F2N64_STACK_GUARD_KIB * 1024)
#define STACK_PAINT 0x5741434bu
void *sbrk_top(int incr);
static uint32_t *stack_guard;

static uint32_t stack_guard_size;
#define STACK_GUARD_KEEP (16 * 1024)

static uint32_t stack_guard_used(void) {
    if (!stack_guard) return 0;
    uint32_t n = stack_guard_size / 4, i = 0;
    while (i < n && stack_guard[i] == STACK_PAINT) i++;
    return (n - i) * 4;
}

static void big_init(void) {
    void *g = sbrk_top(STACK_GUARD);
    if (g != (void *)-1) {
        stack_guard = g;
        stack_guard_size = STACK_GUARD;
        for (uint32_t i = 0; i < STACK_GUARD / 4; i++) stack_guard[i] = STACK_PAINT;
    }
    heap_stats_t s;
    sys_get_heap_stats(&s);
    /* The main heap also gets the stack guard back after boot. */
    int avail = s.total - s.used - F2N64_MAIN_RESERVE_KIB * 1024 + (STACK_GUARD - STACK_GUARD_KEEP);
    if (avail < 512 * 1024) avail = 512 * 1024;
    /* Whatever the allocator's overhead, take the largest block it gives. */
    for (; avail >= 512 * 1024; avail -= 16 * 1024) {
        pool_base = __real_memalign(64, avail);
        if (pool_base) break;
    }
    if (!pool_base) return;
    pool_size = avail & ~15u;
    pool_end = pool_base + pool_size;
    pb *b = (pb *)pool_base;
    b->size = pool_size - 16;
    b->prev = 0;
    pool_free_list = pool_free_tail = fl_hint = NULL;
    pool_runs = 0;
    fl_insert(b);
    pb *end = pb_next(b); /* sentinel: never free */
    end->size = 16 | PB_USED;
    end->prev = PB_SZ(b);
    struct mallinfo mi = mallinfo();
    mem_main_used = mem_main_peak = mi.uordblks - (int)pool_size;
    mem_main_only = 1; /* the boot and the session setup: main heap (Rust: MEM_PLAY once up) */
}

static inline int big_owns(void *p) { return (uint8_t *)p >= pool_base && (uint8_t *)p < pool_end; }

/* `top`: the highest place that fits (play-time blocks), else the lowest (level loads). */
static void *big_alloc(uint32_t req, uint32_t align, int top) {
    if (!pool_base) return NULL;
#ifdef F2N64_POOL_BOTTOM_ONLY
    top = 0; /* testing: everything first fit from the bottom */
#endif
    if (align < 16) align = 16;
    uint32_t need = ((req + 15) & ~15u) + 16;
    pb *best = NULL;
    uintptr_t best_p = 0;
    for (pb *f = top ? pool_free_tail : pool_free_list; f; f = top ? f->pf : f->nf) {
        uintptr_t s = (uintptr_t)f, e = s + PB_SZ(f), p;
        if (PB_SZ(f) < need) continue;
        if (top) {
            p = ((e - need + 16) & ~(uintptr_t)(align - 1));
            if (p - 16 != s && p - 16 < s + PB_MIN) p -= align; /* a gap must hold a block */
            if (p < s + 16 || p - 16 + need > e) continue;
        } else {
            p = (s + 16 + align - 1) & ~(uintptr_t)(align - 1);
            if (p - 16 != s && p - 16 < s + PB_MIN) p += align;
            if (p - 16 + need > e) continue;
        }
        best = f;
        best_p = p;
        break;
    }
    if (!best) return NULL;
    uint32_t total = PB_SZ(best);
    uint32_t gap = (uint32_t)(best_p - 16 - (uintptr_t)best);
    uint32_t tail = total - gap - need;
    if (tail < PB_MIN) {
        need += tail;
        tail = 0;
    }
    pb *b = best, *t = NULL;
    if (gap) { /* the part below stays free, in its place on the list */
        best->size = gap;
        b = (pb *)((uint8_t *)best + gap);
        b->prev = gap;
    }
    b->size = need | PB_USED;
    if (tail) {
        t = (pb *)((uint8_t *)b + need);
        t->size = tail;
        t->prev = need;
        pb_next(t)->prev = tail;
        if (gap) {
            fl_link_after(best, t);
        } else {
            fl_replace(best, t);
        }
    } else {
        pb_next(b)->prev = need;
        if (!gap) fl_unlink(best);
    }
    b->epoch = mem_phase ? mem_epoch : 0;
    b->req = req;
    pool_used += need;
    if (pool_used > pool_peak) pool_peak = pool_used;
    return b + 1;
}

static void big_free(void *ptr) {
    pb *b = (pb *)ptr - 1;
    uint32_t size = PB_SZ(b);
    pool_used -= size;
    pb *n = pb_next(b);
    pb *p = b->prev ? (pb *)((uint8_t *)b - b->prev) : NULL;
    int nfree = !(n->size & PB_USED), pfree = p && !(p->size & PB_USED);
    if (pfree) { /* grows the block below (on the list already) */
        if (nfree) {
            fl_unlink(n);
            size += PB_SZ(n);
        }
        p->size = PB_SZ(p) + size;
        b = p;
    } else if (nfree) { /* takes the place of the block above */
        b->size = size + PB_SZ(n);
        fl_replace(n, b);
    } else {
        b->size = size;
        fl_insert(b);
    }
    pb_next(b)->prev = PB_SZ(b);
}

static uint32_t big_usable(void *ptr) {
    pb *b = (pb *)ptr - 1;
    return PB_SZ(b) - 16 - (uint32_t)(((uintptr_t)ptr - (uintptr_t)(b + 1)));
}

/* Largest free run in the level heap (bytes of payload). */
static uint32_t big_largest(void) {
    uint32_t m = 0;
    for (pb *f = pool_free_list; f; f = f->nf)
        if (PB_SZ(f) > m) m = PB_SZ(f);
    return m > 16 ? m - 16 : 0;
}


static void big_dump(void) {
    pb *prev = NULL;
    for (pb *b = (pb *)pool_base; b && (uint8_t *)b < pool_end && PB_SZ(b) != 16; prev = b, b = pb_next(b)) {
        /* Small blocks between two large free runs: what cuts the free space. */
        if ((b->size & PB_USED) && PB_SZ(b) < 4096 && prev && !(prev->size & PB_USED) && PB_SZ(prev) >= 16384 &&
            !(pb_next(b)->size & PB_USED) && PB_SZ(pb_next(b)) >= 16384)
            debugf("pool SPLIT %p: %lu bytes (req %lu) epoch %lu\n", (void *)(b + 1), (unsigned long)PB_SZ(b), (unsigned long)b->req, (unsigned long)b->epoch);
        if (PB_SZ(b) >= 4096 || !(b->size & PB_USED))
            debugf("pool %s %p: %lu bytes epoch %lu\n", (b->size & PB_USED) ? "block" : "FREE ", (void *)(b + 1),
                   (unsigned long)PB_SZ(b), (unsigned long)((b->size & PB_USED) ? b->epoch : 0));
    }
}

/* MEM_PLAY (0) / MEM_LEVEL (1) / MEM_MAIN (2: session data, main heap whatever the size) /
 * MEM_LEVEL_TOP (3: the renderer's side of a load, from the level heap's top);
 * returns the previous phase. Entering MEM_LEVEL from MEM_PLAY starts
 * a new load epoch (unless | 0x100: back to the current one). */
uint32_t n64_mem_phase_impl(uint32_t phase) {
    uint32_t old = mem_main_only ? 2 : mem_level_top ? 3 : mem_phase;
    uint32_t p = phase & 0xff;
    int level = p == 1 || p == 3;
    if (level && !mem_phase && !(phase & 0x100)) mem_epoch++; /* 0x100: resume the same load */
    mem_phase = level;
    mem_main_only = p == 2;
    mem_level_top = p == 3;
    return old;
}

uint32_t mem_phase_save_and_play(void) { uint32_t o = mem_phase; mem_phase = 0; return o; }
void mem_phase_restore(uint32_t o) { mem_phase = o; } /* same epoch */
/* Session-lifetime allocations (the music stream): main heap whatever their size. */
void mem_main_begin(void) {
    mem_main_save = mem_phase | mem_main_only << 1;
    mem_phase = 0;
    mem_main_only = 1;
}
void mem_main_end(void) {
    mem_phase = mem_main_save & 1;
    mem_main_only = mem_main_save >> 1;
}

/* One `mem` log line: both heaps, and the level heap's blocks by lifetime. `check`: also
 * list the blocks that earlier loads left behind (which must not exist). */
void n64_mem_report_impl(const char *tag, uint32_t len, uint32_t check) {
    if (check & 4) { /* during play: counters only (the block walk below takes ~4 ms) */
        debugf("mem [%.*s]: level heap %lu KiB used (peak %lu) of %lu, %lu free runs | main heap %d KiB used (peak %d) | "
               "fallbacks: play->level %lu, level->main %lu | stack below its 64 KiB: %lu KiB of %d\n",
               (int)len, tag, (unsigned long)(pool_used / 1024), (unsigned long)(pool_peak / 1024), (unsigned long)(pool_size / 1024),
               (unsigned long)pool_runs, mem_main_used / 1024, mem_main_peak / 1024, (unsigned long)mem_fallback_play,
               (unsigned long)mem_fallback_level, (unsigned long)(stack_guard_used() / 1024), (int)(stack_guard_size / 1024));
        return;
    }
    uint32_t cur = 0, cur_n = 0, old = 0, old_n = 0, play = 0, play_n = 0;
    for (pb *b = (pb *)pool_base; b && (uint8_t *)b < pool_end - 16; b = pb_next(b)) {
        if (!(b->size & PB_USED)) continue;
        if (b->epoch == 0) play += PB_SZ(b), play_n++;
        else if (b->epoch == mem_epoch) cur += PB_SZ(b), cur_n++;
        else old += PB_SZ(b), old_n++;
    }
    int main_used = mem_main_used;
    heap_stats_t s;
    sys_get_heap_stats(&s);
    int main_cap = s.total - (int)pool_size;
    debugf("mem [%.*s]: level heap %lu KiB used (peak %lu) of %lu: this load %lu KiB/%lu blocks, earlier loads %lu KiB/%lu, "
           "play %lu KiB/%lu; free %lu KiB in %lu runs, largest %lu KiB | main heap %d KiB used (peak %d) of %d | "
           "fallbacks: play->level %lu, level->main %lu (%lu KiB) | stack below its 64 KiB: %lu KiB of %d\n",
           (int)len, tag, (unsigned long)(pool_used / 1024), (unsigned long)(pool_peak / 1024), (unsigned long)(pool_size / 1024),
           (unsigned long)(cur / 1024), (unsigned long)cur_n, (unsigned long)(old / 1024), (unsigned long)old_n,
           (unsigned long)(play / 1024), (unsigned long)play_n, (unsigned long)((pool_size - 16 - pool_used) / 1024),
           (unsigned long)pool_runs, (unsigned long)(big_largest() / 1024), main_used / 1024, mem_main_peak / 1024, main_cap / 1024,
           (unsigned long)mem_fallback_play, (unsigned long)mem_fallback_level, (unsigned long)(mem_fallback_level_bytes / 1024),
           (unsigned long)(stack_guard_used() / 1024), (int)(stack_guard_size / 1024));
    if ((check & 2) && stack_guard) { /* boot done: keep a small guard, give the rest back to the heap */
        if (stack_guard_size > STACK_GUARD_KEEP && sbrk_top(-(int)(stack_guard_size - STACK_GUARD_KEEP)) != (void *)-1) {
            stack_guard = (uint32_t *)((uint8_t *)stack_guard + stack_guard_size - STACK_GUARD_KEEP);
            stack_guard_size = STACK_GUARD_KEEP;
        }
        for (uint32_t i = 0; i < stack_guard_size / 4; i++) stack_guard[i] = STACK_PAINT;
    }
    if ((check & 1) && play_n) /* play-time blocks still alive after a load: session data, or a leak */
        for (pb *b = (pb *)pool_base; b && (uint8_t *)b < pool_end - 16; b = pb_next(b))
            if ((b->size & PB_USED) && !b->epoch && PB_SZ(b) >= 1024)
                debugf("mem: play-time block alive across a load: %p %lu bytes\n", (void *)(b + 1), (unsigned long)b->req);
    if ((check & 1) && old_n) {
        int shown = 0;
        for (pb *b = (pb *)pool_base; b && (uint8_t *)b < pool_end - 16; b = pb_next(b))
            if ((b->size & PB_USED) && b->epoch && b->epoch != mem_epoch && shown++ < 24)
                debugf("mem: LEFT OVER from load %lu: %p %lu bytes\n", (unsigned long)b->epoch, (void *)(b + 1), (unsigned long)b->req);
    }
}

/* Peaks restart (per level). */
void n64_mem_reset_peaks_impl(void) {
    pool_peak = pool_used;
    mem_main_peak = mem_main_used;
}

/* The whole program's malloc family is routed here (`ld --wrap`, tools/n64/Makefile): Rust,
 * our C host and libdragon (asset_load, surfaces, the H.264 decoder's frame buffers, ...). */
void *__wrap_memalign(size_t align, size_t size) {
#ifdef F2N64_BIGPOOL_DUMP
    if (size >= 200000) {
        void *bt[8];
        int n = backtrace(bt, 8);
        debugf("bigalloc %lu:", (unsigned long)size);
        for (int i = 0; i < n; i++) debugf(" %p", bt[i]);
        debugf("\n");
    }
#endif
    if (!size) size = 1;
    if (mem_phase && align <= 64) {
        void *p = big_alloc(size, align, mem_level_top);
#ifdef F2N64_MEM_TRACE_SIZE
        if (p && (size == F2N64_MEM_TRACE_SIZE || size == F2N64_MEM_TRACE_SIZE2)) { /* testing: who allocates it */
            void *bt[14];
            int n = backtrace(bt, 14);
            debugf("memtrace %lu %p:", (unsigned long)size, p);
            for (int i = 4; i < n; i++) debugf(" %p", bt[i]);
            debugf("\n");
        }
#endif
        if (p) return p;
        if (mem_fallback_level++ == 0 || size >= 32 * 1024)
            debugf("mem: LEVEL HEAP FULL: %lu bytes from the main heap (level heap %lu KiB used of %lu, largest free %lu KiB)\n",
                   (unsigned long)size, (unsigned long)(pool_used / 1024), (unsigned long)(pool_size / 1024), (unsigned long)(big_largest() / 1024));
        mem_fallback_level_bytes += size;
    }
    /* Large play-time blocks are level data too in practice (the renderer's per-level storage
     * growing in the first frames, the story book page, procedural texture variants): the
     * level heap's top, freed with the level, so the main heap keeps the session's small
     * blocks. (Session-lifetime large blocks allocated in play are wrapped in MEM_PLAY_MAIN:
     * the music stream.) */
#ifdef F2N64_MEM_TRACE_SIZE
    if (!mem_phase && size >= F2N64_MEM_TRACE_PLAY) { /* testing: large play-time blocks */
        void *bt[14];
        int n = backtrace(bt, 14);
        debugf("memtrace play %lu:", (unsigned long)size);
        for (int i = 4; i < n; i++) debugf(" %p", bt[i]);
        debugf("\n");
    }
#endif
    if (!mem_phase && size >= PLAY_BIG && align <= 64 && !mem_main_only) {
        void *q = big_alloc(size, align, 1);
        if (q) return q;
    }
    void *p = __real_memalign(align < 8 ? 8 : align, size);
    if (p) main_count(p, 1);
    if (!p && !mem_phase && align <= 64) { /* main heap full: the top of the level heap */
        p = big_alloc(size, align, 1);
        if (p && (mem_fallback_play++ & 1023) == 0)
            debugf("mem: main heap full, play-time blocks from the level heap's top (%lu so far, level heap %lu KiB used)\n",
                   (unsigned long)mem_fallback_play, (unsigned long)(pool_used / 1024));
    }
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
    else {
        main_count(ptr, -1);
        __real_free(ptr);
    }
}

void *__wrap_realloc(void *ptr, size_t size) {
    if (!ptr) return __wrap_malloc(size);
    if (!size) {
        __wrap_free(ptr);
        return NULL;
    }
    uint32_t have = big_owns(ptr) ? big_usable(ptr) : malloc_usable_size(ptr);
    if (big_owns(ptr) && size <= have) return ptr;
    if (!big_owns(ptr)) { /* a main-heap block stays there when it can (session buffers growing during a load) */
        main_count(ptr, -1);
        void *r = __real_realloc(ptr, size);
        main_count(r ? r : ptr, 1);
        if (r) return r;
    }
    /* Across heaps (or the main heap is full): move. */
    void *n = __wrap_malloc(size);
    if (n) {
        memcpy(n, ptr, have < size ? have : size);
        __wrap_free(ptr);
    }
    return n;
}

void *n64_alloc_impl(uint32_t size, uint32_t align) {
    void *p = memalign(align < 8 ? 8 : align, size ? size : 1);
    if (!p) {
        /* The allocator's own view (heap stats count free holes as free). */
        extern char *__heap_end, *__heap_top;
        extern int __heap_total_size, __heap_top_allocated_size;
        struct mallinfo m = mallinfo();
        debugf("malloc: arena %d KiB, in use %d KiB, free holes %d KiB (%d chunks), top chunk %d KiB; "
               "sbrk end %p top %p (%ld KiB left), top-allocated %d KiB, heap %d KiB; pool %lu/%d KiB\n",
               m.arena / 1024, m.uordblks / 1024, m.fordblks / 1024, m.ordblks, m.keepcost / 1024,
               __heap_end, __heap_top, (long)(__heap_top - __heap_end) / 1024, __heap_top_allocated_size / 1024,
               __heap_total_size / 1024, (unsigned long)(pool_used / 1024), (int)(pool_size / 1024));
    }
    return p;
}

void n64_free_impl(void *ptr) { free(ptr); }

uint32_t n64_ticks_impl(void) { return TICKS_READ(); }

uint32_t n64_memory_size_impl(void) { return get_memory_size(); }

void n64_heap_stats_impl(n64_heap_t *out) {
    heap_stats_t s;
    sys_get_heap_stats(&s);
    out->total = s.total;
    /* The pool is one reserved block of the main heap: count what is used inside it. */
    out->used = pool_base ? s.used - (int)pool_size + (int)pool_used : s.used;
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
    if (!p) big_dump();
    assertf(p, "Out of memory reading %s (%d bytes; level heap %lu/%lu KiB)", path, sz, (unsigned long)(pool_used / 1024), (unsigned long)(pool_size / 1024));
    /* 32 KiB at a time with the music topped up in between: a level's large files (texture
     * banks, compressed) take a few hundred ms to read and inflate, longer than the audio
     * buffer (300 ms): the music dropped out while loading. */
    int got = 0;
    while (got < sz) {
        int want = sz - got < 32768 ? sz - got : 32768;
        int n = fread(p + got, 1, want, f);
        if (n <= 0) break;
        got += n;
        n64_audio_poll_impl();
    }
    fclose(f);
    if (len) *len = (uint32_t)got;
    return p;
}

uint32_t n64_bigpool_peak_kib(void) { return pool_peak / 1024; }

/* ---- textures ---- */
/* Columns x0 .. x0+w of an image description as a new surface. */
static surface_t make_surface_cols(const n64_tex_desc_t *d, uint32_t x0, uint32_t w) {
    tex_format_t fmt = (d->fmt & 0xff) == 1 ? FMT_RGBA32 : FMT_RGBA16;
    surface_t s = surface_alloc(fmt, w, d->height);
    if (!s.buffer) return s; /* out of memory: the caller frees and fails */
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
        /* Out of memory (a story book page grabbed in a full frontend crashed in memcpy to
         * NULL): undo and fail; the caller decides (texture missing, or a smaller one). */
        bool ok = t->full != NULL && t->nfull > 0 && t->small.buffer != NULL;
        for (int k = 0; ok && k < t->nfull; k++) ok = t->full[k].buffer != NULL;
        if (!ok) {
            debugf("tex: out of memory creating a %lux%lu texture\n", (unsigned long)full->width, (unsigned long)full->height);
            if (small && t->small.buffer) surface_free(&t->small);
            if (t->full) {
                for (int k = 0; k < t->nfull; k++)
                    if (t->full[k].buffer) surface_free(&t->full[k]);
                if (t->full != &t->full1) free(t->full);
            }
            t->full = NULL;
            t->nfull = 0;
            return 0;
        }
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
        t->tiled = (d->flags & N64_NTX_TILED) && t->tile3d;
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
    /* surface_alloc buffers are uncached; others need a writeback for the RDP. (Writing an
     * uncached one through its cached alias + writeback was slower: write-allocate misses.) */
    if (!((uint32_t)s->buffer & 0x20000000)) data_cache_hit_writeback(s->buffer, s->stride * s->height);
    if (t->has_addv) make_add_variant_from(t, d->pixels, row);
    if (cur_tex_loaded == h) cur_tex_loaded = 0;
    return 1;
}

void *n64_tex_update_begin_impl(uint32_t h, const n64_tex_desc_t *d) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return NULL;
    tex_t *t = &textures[h];
    tex_format_t fmt = (d->fmt & 0xff) == 1 ? FMT_RGBA32 : FMT_RGBA16;
    if (!t->small_is_full || t->nfull != 1 || t->tile3d || t->external || t->own_small) return NULL;
    surface_t *s = &t->full[0];
    if (s->width != d->width || s->height != d->height || surface_get_format(s) != fmt) return NULL;
    if (s->stride != TEX_FORMAT_PIX2BYTES(fmt, d->width) || !((uint32_t)s->buffer & 0x20000000)) return NULL;
    return s->buffer;
}

void n64_tex_update_end_impl(uint32_t h) {
    if (h == 0 || h >= MAX_TEXTURES || !textures[h].used) return;
    tex_t *t = &textures[h];
    if (t->has_addv) make_add_variant(t);
    if (cur_tex_loaded == h) cur_tex_loaded = 0;
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
    if ((t->own_small || !t->small_is_full) && t->small.buffer != t->full[0].buffer && !(t->has_tmem && t->small.buffer == t->tmem.buffer))
        surface_free(&t->small);
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
    uint32_t t0 = TICKS_READ();
    /* The TMEM copy r3d asked for is the same downscale of the same image: share it. */
    if (t->has_tmem && t->small_is_full && t->small.buffer == t->full[0].buffer) t->small = t->tmem;
    else t->small = downscale_rgba32(t, &t->full[0]);
    uint32_t dt = TICKS_READ() - t0;
    if (dt > TICKS_FROM_US(200))
        debugf("cpu texture: downscaled copy of %dx%d fmt %d in %lu us\n", t->full[0].width, t->full[0].height, (int)surface_get_format(&t->full[0]), (unsigned long)TICKS_TO_US(dt));
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
        uint32_t t0 = TICKS_READ();
        t->tmem = downscale_rgba32(t, &t->small);
        t->has_tmem = true;
        uint32_t dt = TICKS_READ() - t0;
        if (dt > TICKS_FROM_US(200))
            debugf("cpu texture: TMEM copy of %dx%d fmt %d (%d) in %lu us\n", t->small.width, t->small.height, (int)surface_get_format(&t->small), (int)h, (unsigned long)TICKS_TO_US(dt));
    }
    return &t->tmem;
}

/* A box-downscaled RGBA32 copy of surface s of texture t, at most 1024 texels. */
/* Texel decoding for the CPU-side conversions (downscaled copies, additive variants), as
 * tex_fetch but packed RGBA8 (r << 24 | g << 16 | b << 8 | a): 4- and 8-bit formats through
 * a table of their 16 / 256 values (tex_fetch per value), RGBA16 through a 5-bit table.
 * tex_fetch per texel (a switch, divisions, byte loads) cost ~2 us per texel: 35 ms for the
 * HUD clock's 128x128 CI8 hand on its first draw. */
typedef struct { tex_format_t f; bool four, byte; uint32_t lut[256]; } px_decoder_t;
static inline uint32_t pack_px(const uint32_t px[4]) { return px[0] << 24 | px[1] << 16 | px[2] << 8 | px[3]; }
static void px_decoder_init(px_decoder_t *d, const tex_t *t, const surface_t *s) {
    d->f = surface_get_format(s);
    d->four = d->f == FMT_CI4 || d->f == FMT_IA4 || d->f == FMT_I4;
    d->byte = d->f == FMT_CI8 || d->f == FMT_IA8 || d->f == FMT_I8;
    if (d->four || d->byte) {
        uint8_t one[2] = {0, 0};
        surface_t probe = surface_make_linear(one, d->f, 1, 1);
        for (int v = 0; v < (d->four ? 16 : 256); v++) {
            uint32_t px[4];
            one[0] = d->four ? (uint8_t)(v << 4) : (uint8_t)v;
            tex_fetch(t, &probe, 0, 0, px);
            d->lut[v] = pack_px(px);
        }
    } else if (d->f != FMT_RGBA32 && d->f != FMT_IA16) {
        for (int v = 0; v < 32; v++) d->lut[v] = v * 255 / 31;
    }
}
static inline uint32_t px_decode(const px_decoder_t *d, const tex_t *t, const surface_t *s, const uint8_t *row, int x) {
    if (d->four) return d->lut[(row[x / 2] >> ((x & 1) ? 0 : 4)) & 15];
    if (d->byte) return d->lut[row[x]];
    /* Whole-texel loads: converted surfaces are uncached, one access per texel (big endian:
     * the byte order of the texel). */
    if (d->f == FMT_RGBA32) {
        if (!((uintptr_t)row & 3)) return ((const uint32_t *)row)[x];
        return (uint32_t)row[x * 4] << 24 | row[x * 4 + 1] << 16 | row[x * 4 + 2] << 8 | row[x * 4 + 3];
    }
    uint32_t v = ((uintptr_t)row & 1) ? (uint32_t)(row[x * 2] << 8) | row[x * 2 + 1] : ((const uint16_t *)row)[x];
    if (d->f == FMT_IA16) return (v >> 8) * 0x01010100u | (v & 255);
    return d->lut[(v >> 11) & 31] << 24 | d->lut[(v >> 6) & 31] << 16 | d->lut[(v >> 1) & 31] << 8 | ((v & 1) ? 255 : 0);
}

static surface_t downscale_rgba32(tex_t *t, const surface_t *s) {
    int w = s->width, h = s->height, sx = 1, sy = 1;
    while ((w / sx) * (h / sy) > 1024) {
        if (w / sx >= h / sy) sx *= 2; else sy *= 2;
    }
    int nw = w / sx > 0 ? w / sx : 1, nh = h / sy > 0 ? h / sy : 1;
    surface_t d = surface_alloc(FMT_RGBA32, nw, nh);
    px_decoder_t dec; /* (stack: no .bss for rare conversions) */
    px_decoder_init(&dec, t, s);
    if ((dec.four || dec.byte) && nw * sx == w && nh * sy == h && sx * sy <= 256) {
        /* Palette / intensity formats, whole boxes (the usual case): table lookups summed
         * two channels per 32-bit add (16-bit lanes: at most 256 * 255). */
        int sh = __builtin_ctz(sx * sy);
        const uint32_t *lut = dec.lut;
        const bool four = dec.four;
        int lsx = __builtin_ctz(sx);
        for (int y = 0; y < nh; y++) {
            uint32_t *o = (uint32_t *)((uint8_t *)d.buffer + y * d.stride);
            for (int x = 0; x < nw; x++) {
                uint32_t rb = 0, ga = 0;
                for (int yy = y * sy; yy < (y + 1) * sy; yy++) {
                    const uint8_t *row = (const uint8_t *)s->buffer + yy * s->stride;
                    for (int xx = x << lsx; xx < (x + 1) << lsx; xx++) {
                        uint32_t p = four ? lut[(row[xx >> 1] >> ((xx & 1) ? 0 : 4)) & 15] : lut[row[xx]];
                        rb += (p >> 8) & 0x00FF00FF; /* r, b */
                        ga += p & 0x00FF00FF;        /* g, a */
                    }
                }
                o[x] = ((rb >> 16 >> sh) & 255) << 24 | ((ga >> 16 >> sh) & 255) << 16 | ((rb & 0xFFFF) >> sh) << 8 | ((ga & 0xFFFF) >> sh);
            }
        }
        return d;
    }
    for (int y = 0; y < nh; y++) {
        uint32_t *o = (uint32_t *)((uint8_t *)d.buffer + y * d.stride);
        for (int x = 0; x < nw; x++) {
            uint32_t acc[4] = {0, 0, 0, 0}, n = 0;
            for (int yy = y * sy; yy < (y + 1) * sy && yy < h; yy++) {
                const uint8_t *row = (const uint8_t *)s->buffer + yy * s->stride;
                for (int xx = x * sx; xx < (x + 1) * sx && xx < w; xx++) {
                    uint32_t p = px_decode(&dec, t, s, row, xx);
                    acc[0] += p >> 24; acc[1] += (p >> 16) & 255; acc[2] += (p >> 8) & 255; acc[3] += p & 255;
                    n++;
                }
            }
            /* One 32-bit store per texel (the buffer is uncached); n is a power of two
             * except at clipped edges. */
            uint32_t q[4];
            if (n && !(n & (n - 1))) {
                int sh = __builtin_ctz(n);
                for (int k = 0; k < 4; k++) q[k] = acc[k] >> sh;
            } else {
                for (int k = 0; k < 4; k++) q[k] = n ? acc[k] / n : 0;
            }
            o[x] = pack_px(q);
        }
    }
    return d;
}

static void make_add_variant_from(tex_t *t, const void *src, uint32_t src_stride);
static void make_add_variant(tex_t *t) {
    uint32_t t0 = TICKS_READ();
    make_add_variant_from(t, NULL, 0);
    uint32_t dt = TICKS_READ() - t0;
    if (dt > TICKS_FROM_US(500))
        debugf("cpu texture: additive variant of %dx%d fmt %d in %lu us\n", t->small.width, t->small.height, (int)surface_get_format(&t->small), (unsigned long)TICKS_TO_US(dt));
}

/* `src` (optional): the small surface's texels in cached memory (`src_stride` bytes per
 * row; the procedural refill's source), read instead of the uncached surface. */
static void make_add_variant_from(tex_t *t, const void *src, uint32_t src_stride) {
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
        /* surface_alloc memory is uncached: one 32-bit store per texel. */
        uint8_t *dbase = (uint8_t *)t->addv.buffer;
        for (int y = 0; y < nh; y++) {
            const uint8_t *row = src ? (const uint8_t *)src + y * src_stride : (const uint8_t *)s->buffer + y * s->stride;
            uint32_t *dst = (uint32_t *)(dbase + y * t->addv.stride);
            for (int x = 0; x < nw; x++) {
                uint32_t r, g, b, a;
                /* One load per texel when aligned (the surfaces are uncached). */
                if (sf == FMT_RGBA32) {
                    uint32_t p = ((uintptr_t)row & 3) ? (uint32_t)row[x * 4] << 24 | row[x * 4 + 1] << 16 | row[x * 4 + 2] << 8 | row[x * 4 + 3]
                                                      : ((const uint32_t *)row)[x];
                    r = p >> 24; g = (p >> 16) & 255; b = (p >> 8) & 255; a = p & 255;
                } else {
                    uint32_t v = ((uintptr_t)row & 1) ? (uint32_t)(row[x * 2] << 8) | row[x * 2 + 1] : ((const uint16_t *)row)[x];
                    r = ((v >> 11) & 31) * 255 / 31; g = ((v >> 6) & 31) * 255 / 31; b = ((v >> 1) & 31) * 255 / 31;
                    a = (v & 1) ? 255 : 0;
                }
                /* x / 255 for x <= 255 * 255: (x * 0x8081) >> 23 (exact over that range). */
                r = (r * a * 0x8081) >> 23; g = (g * a * 0x8081) >> 23; b = (b * a * 0x8081) >> 23;
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
    bool small_fmt = sf == FMT_CI4 || sf == FMT_CI8 || sf == FMT_IA4 || sf == FMT_IA8 || sf == FMT_I4 || sf == FMT_I8;
    if (sx == 1 && sy == 1 && !src && small_fmt) {
        /* Palette / intensity formats (NTX textures): the general path's result for each of
         * the 16 or 256 texel values, then one table lookup and one 32-bit store per texel.
         * The general path took ~5 us per texel (three divisions, four uncached byte
         * stores): a 6-8 ms stall on an effect's first additive use. Same values. */
        bool four = sf == FMT_CI4 || sf == FMT_IA4 || sf == FMT_I4;
        uint32_t lut[256];
        uint8_t one[2];
        surface_t probe = surface_make_linear(one, sf, 1, 1);
        for (int v = 0; v < (four ? 16 : 256); v++) {
            one[0] = four ? (uint8_t)(v << 4) : (uint8_t)v;
            uint32_t px[4];
            tex_fetch(t, &probe, 0, 0, px);
            uint32_t r = px[0] * px[3] / 255, g = px[1] * px[3] / 255, b = px[2] * px[3] / 255;
            uint32_t m = r > g ? (r > b ? r : b) : (g > b ? g : b);
            lut[v] = m ? (r * 255 / m) << 24 | (g * 255 / m) << 16 | (b * 255 / m) << 8 | m : 0;
        }
        uint8_t *dbase = (uint8_t *)t->addv.buffer;
        for (int y = 0; y < nh; y++) {
            const uint8_t *row = (const uint8_t *)s->buffer + y * s->stride;
            uint32_t *dst = (uint32_t *)(dbase + y * t->addv.stride);
            if (four)
                for (int x = 0; x < nw; x++) dst[x] = lut[(row[x / 2] >> ((x & 1) ? 0 : 4)) & 15];
            else
                for (int x = 0; x < nw; x++) dst[x] = lut[row[x]];
        }
        t->has_addv = true;
        return;
    }
    px_decoder_t dec;
    px_decoder_init(&dec, t, s);
    /* Palette / intensity formats: the premultiplied channels per texel value (no multiply
     * per texel: ~25 cycles each in ares). */
    uint32_t prem[3][256];
    bool table = dec.four || dec.byte;
    if (table)
        for (int v = 0; v < (dec.four ? 16 : 256); v++) {
            uint32_t p = dec.lut[v], a = p & 255;
            prem[0][v] = (p >> 24) * a; prem[1][v] = ((p >> 16) & 255) * a; prem[2][v] = ((p >> 8) & 255) * a;
        }
    for (int y = 0; y < nh; y++) {
        uint32_t *dst = (uint32_t *)((uint8_t *)t->addv.buffer + y * t->addv.stride);
        for (int x = 0; x < nw; x++) {
            uint32_t acc[4] = {0, 0, 0, 0}, n = 0;
            for (int yy = y * sy; yy < (y + 1) * sy && yy < h; yy++) {
                const uint8_t *row = (const uint8_t *)s->buffer + yy * s->stride;
                for (int xx = x * sx; xx < (x + 1) * sx && xx < w; xx++) {
                    if (table) {
                        uint32_t v = dec.four ? (row[xx / 2] >> ((xx & 1) ? 0 : 4)) & 15 : row[xx];
                        acc[0] += prem[0][v]; acc[1] += prem[1][v]; acc[2] += prem[2][v]; n++;
                        continue;
                    }
                    uint32_t p = px_decode(&dec, t, s, row, xx);
                    uint32_t r = p >> 24, g = (p >> 16) & 255, b = (p >> 8) & 255, a = p & 255;
                    /* premultiplied accumulation: what the additive mode adds is rgb*a */
                    acc[0] += r * a; acc[1] += g * a; acc[2] += b * a; n++;
                }
            }
            uint32_t r, g, b;
            if (n && !(n & (n - 1))) {
                /* x / (255 n) = (x >> log2 n) / 255, and y / 255 = (y * 0x8081) >> 23 for
                 * y <= 255 * 255 (exact, as the fast path above). */
                int sh = __builtin_ctz(n);
                r = ((acc[0] >> sh) * 0x8081) >> 23; g = ((acc[1] >> sh) * 0x8081) >> 23; b = ((acc[2] >> sh) * 0x8081) >> 23;
            } else {
                r = acc[0] / (255 * n), g = acc[1] / (255 * n), b = acc[2] / (255 * n);
            }
            uint32_t m = r > g ? (r > b ? r : b) : (g > b ? g : b);
            uint32_t o = m;
            if (m) {
                /* r * 255 / m by reciprocal (exact: numerators < 2^16, as the fast path). */
                uint64_t k = (uint64_t)(0xFFFFFFFFu / m) + 1;
                o |= (uint32_t)(((uint64_t)(r * 255) * k) >> 32) << 24;
                o |= (uint32_t)(((uint64_t)(g * 255) * k) >> 32) << 16;
                o |= (uint32_t)(((uint64_t)(b * 255) * k) >> 32) << 8;
            }
            dst[x] = o;
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

/* ---- texture uploads recorded once (phase 3E) ----
 * rdpq_tex_upload / _sub cost ~50 us of CPU each (tex_loader set-up, autosync, ~6 RDP
 * commands); replaying the same commands from a recorded rspq block costs a few us. The
 * commands depend only on the surface (address, format, size, stride), the sub-rectangle,
 * the tile and the repeat flag, which form the key: a stale entry whose buffer was freed
 * and reused for an identical surface records exactly the commands needed. Evicted blocks
 * are freed UPC_GRACE presented frames later (the RSP may still be running them). */
#define UPC_N 128
#define UPC_GRACE 4
typedef struct {
    const void *buf;
    uint32_t key; /* fmt | wrap | tile */
    uint16_t w, h, stride, x0, y0, x1, y1;
    rspq_block_t *blk;
} upc_entry_t;
static upc_entry_t upc[UPC_N];
#define UPC_RETIRE 64
static struct { rspq_block_t *blk; uint32_t frame; } upc_retired[UPC_RETIRE];
static uint32_t upc_nretired, upc_frame, st_upc_hits, st_upc_misses, st_upc_direct;

static void upc_collect(void) {
    uint32_t k = 0;
    for (uint32_t i = 0; i < upc_nretired; i++) {
        if (upc_frame - upc_retired[i].frame >= UPC_GRACE) rspq_block_free(upc_retired[i].blk);
        else upc_retired[k++] = upc_retired[i];
    }
    upc_nretired = k;
}

/* rdpq_tex_upload_sub(tile, s, wrap ? repeat : NULL, x0, y0, x1, y1) through the cache. */
static void tex_upload_cached(rdpq_tile_t tile, const surface_t *s, bool wrap, int x0, int y0, int x1, int y1) {
    uint32_t key = (uint32_t)surface_get_format(s) | (wrap ? 0x10000 : 0) | ((uint32_t)tile << 20);
    uint32_t hsh = ((uint32_t)s->buffer >> 3) ^ (uint32_t)(x0 * 7 + y0 * 131 + x1 * 17 + y1 * 257) ^ key;
    hsh ^= hsh >> 11;
    /* 4-way set associative (UPC_N / 4 sets), round-robin replacement. */
    upc_entry_t *set = &upc[((hsh * 2654435761u) >> 27) * 4];
    for (int w = 0; w < 4; w++) {
        upc_entry_t *e = &set[w];
        if (e->blk && e->buf == s->buffer && e->key == key && e->w == s->width && e->h == s->height && e->stride == s->stride &&
            e->x0 == x0 && e->y0 == y0 && e->x1 == x1 && e->y1 == y1) {
            st_upc_hits++;
            rspq_block_run(e->blk);
            return;
        }
    }
    st_upc_misses++;
    rdpq_texparms_t parms = {0};
    if (wrap) parms.s.repeats = parms.t.repeats = REPEAT_INFINITE;
    /* Second chance: a sub-rect seen for the first time (a glyph of new text) is uploaded
     * directly; recording it as a block (and evicting a block in use) only pays off when it
     * comes back. */
    static uint32_t ghost[32];
    static uint8_t ghost_i;
    uint32_t gk = hsh * 2654435761u + (uint32_t)(x1 - x0) * 40503u + (uint32_t)(y1 - y0);
    bool seen = false;
    for (int g = 0; g < 32; g++)
        if (ghost[g] == gk) { seen = true; ghost[g] = 0; break; }
    if (!seen) {
        ghost[ghost_i++ & 31] = gk;
        st_upc_direct++;
        rdpq_tex_upload_sub(tile, s, &parms, x0, y0, x1, y1);
        return;
    }
    static uint8_t rr;
    upc_entry_t *e = &set[rr++ & 3];
    if (e->blk) {
        if (upc_nretired == UPC_RETIRE) {
            rspq_wait(); /* everything recorded so far has run: free all */
            upc_frame += UPC_GRACE;
            upc_collect();
        }
        upc_retired[upc_nretired].blk = e->blk;
        upc_retired[upc_nretired++].frame = upc_frame;
        e->blk = NULL;
    }
    rspq_block_begin();
    rdpq_tex_upload_sub(tile, s, &parms, x0, y0, x1, y1);
    e->blk = rspq_block_end();
    e->buf = s->buffer;
    e->key = key;
    e->w = s->width;
    e->h = s->height;
    e->stride = s->stride;
    e->x0 = x0;
    e->y0 = y0;
    e->x1 = x1;
    e->y1 = y1;
    rspq_block_run(e->blk);
}

static void set_mode(const n64_state_t *st, int kind, bool tex);
static void set_mode_cached(const n64_state_t *st, int kind, bool tex);
static void apply_state(const n64_state_t *st, int kind) {
    bool tex = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used;
    uint32_t mt0 = TICKS_READ();
    if (cur_kind != kind || st->flags != cur_state.flags || st->blend != cur_state.blend ||
        st->alpha_ref != cur_state.alpha_ref || (st->tex == 0) != (cur_state.tex == 0)) {
        st_modes++;
        set_mode_cached(st, kind, tex);
        cur_tex_loaded = 0;
        cur_tlut_mode = 0; /* rdpq_set_mode_standard: no TLUT */
        rect_tmem = NULL;
    }
    if (kind == 0 && tex && (st->tex != cur_tex_loaded || ((st->flags ^ cur_state.flags) & N64_WRAP))) {
        const surface_t *s = tri_surface(st);
        use_tlut(&textures[st->tex], s, 0);
        uint32_t tt0 = TICKS_READ();
        tex_upload_cached(TILE0, s, (st->flags & N64_WRAP) != 0, 0, 0, s->width, s->height);
        st_tex_ticks += TICKS_READ() - tt0;
        cur_tex_loaded = st->tex;
    }
    if (kind == 1) cur_tex_loaded = 0; /* blits reload TMEM */
    cur_state = *st;
    cur_kind = kind;
    st_mode_ticks += TICKS_READ() - mt0;
}

/* The render mode of a CPU batch (apply_state). */
static void set_mode(const n64_state_t *st, int kind, bool tex) {
        rdpq_set_mode_standard();
        if (kind == 0)
            rdpq_mode_combiner(tex ? RDPQ_COMBINER_TEX_SHADE : RDPQ_COMBINER_SHADE);
        else
            rdpq_mode_combiner(tex ? RDPQ_COMBINER_TEX_FLAT : RDPQ_COMBINER_FLAT);
        rdpq_mode_zbuf(st->flags & N64_ZTEST, st->flags & N64_ZWRITE);
        /* Rectangles have no per-pixel depth: a Z-tested one uses the primitive depth
         * (set per rect, draw_rects_inner). */
        if (kind == 1 && (st->flags & N64_ZTEST)) rdpq_mode_zoverride(true, 0.0f, 0);
        rdpq_mode_filter(st->flags & N64_BILINEAR ? FILTER_BILINEAR : FILTER_POINT);
        rdpq_mode_persp(kind == 0 && (st->flags & N64_PERSP));
        rdpq_mode_alphacompare((int)st->alpha_ref);
        switch (st->blend) {
        case N64_BLEND_SEMI: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        /* Never RDPQ_BLENDER_ADDITIVE (wraps): see make_add_variant. */
        case N64_BLEND_ADD: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        /* D3D ZERO/INVSRCCOLOR (dst * (1 - src)): the blender only has alpha factors, so
         * render_cpu.rs sends black with alpha = the colour's brightness (subtractive_colour):
         * black*a + dst*(1-a), exact for the grey screen fades. */
        case N64_BLEND_SUB: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
        default: rdpq_mode_blender(0); break;
        }
}

/* set_mode recorded once per distinct mode into an rspq block and replayed (~100 us of
 * rdpq mode API calls per reset otherwise; 6-7 resets a frame, after each r3d list).
 * The key holds everything set_mode reads. */
#define MODE_CACHE_N 48
static struct { uint32_t key; rspq_block_t *blk; } mode_cache[MODE_CACHE_N];
static uint32_t mode_cache_n;
static void set_mode_cached(const n64_state_t *st, int kind, bool tex) {
    uint32_t key = (uint32_t)kind | (tex ? 2 : 0) | ((st->blend & 7) << 2) |
                   ((st->flags & (N64_ZTEST | N64_ZWRITE | N64_BILINEAR | N64_PERSP)) << 8) | ((st->alpha_ref & 0xff) << 24);
    for (uint32_t i = 0; i < mode_cache_n; i++)
        if (mode_cache[i].key == key) {
            rspq_block_run(mode_cache[i].blk);
            return;
        }
    if (mode_cache_n == MODE_CACHE_N) {
        set_mode(st, kind, tex);
        return;
    }
    rspq_block_begin();
    set_mode(st, kind, tex);
    rspq_block_t *b = rspq_block_end();
    mode_cache[mode_cache_n].key = key;
    mode_cache[mode_cache_n++].blk = b;
    rspq_block_run(b);
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
static uint32_t st_calls, st_src_tris, st_rdpq_tris, st_tiled_src, st_tiled_uploads, st_frames, st_fast_vtx;

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

/* Per-batch triangle set-up of draw_tris_inner: the rdpq format and how the vertices are
 * sampled (0 = as given, 1 = UVs scaled to the TMEM-sized surface, 2 = TMEM tiles). */
typedef struct { const rdpq_trifmt_t *f; int mode; float sw, sh; const tex_t *tx; uint32_t flags; } tri_setup_t;

static tri_setup_t tri_begin(uint32_t ntris, const n64_state_t *st) {
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
    tri_setup_t su = {f, 0, 1.0f, 1.0f, tex ? &textures[st->tex] : NULL, st->flags};
    if (tex && (st->flags & N64_UVNORM) && textures[st->tex].tile3d && (!textures[st->tex].own_small || textures[st->tex].tiled) && !(st->flags & N64_WRAP) && st->blend != N64_BLEND_ADD) {
        su.mode = 2;
        st_tiled_src += ntris;
    } else if (tex && (st->flags & N64_UVNORM)) {
        /* Normalised UVs: scale to the TMEM-sized surface. */
        const surface_t *s = tri_surface(st);
        su.mode = 1;
        su.sw = s->width;
        su.sh = s->height;
    }
    return su;
}

/* One triangle (vertices may be modified: UV scaling). */
static inline void tri_one(const tri_setup_t *su, n64_vertex_t *t) {
    if (su->mode == 2) {
        draw_tri_tiled(su->f, su->tx, t, su->flags);
        return;
    }
    if (su->mode == 1) {
        for (int j = 0; j < 3; j++) {
            t[j].s *= su->sw;
            t[j].t *= su->sh;
        }
    }
    st_rdpq_tris++;
    rdpq_triangle(su->f, (const float *)&t[0], (const float *)&t[1], (const float *)&t[2]);
}

static void draw_tris_inner(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st);
void n64_draw_tris_impl(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    uint32_t t0 = TICKS_READ(), w0 = rspq_wait_ticks;
    draw_tris_inner(v, ntris, st);
    if (TICKS_READ() - t0 > TICKS_FROM_MS(2)) slow_batch("triangles (vertices)", ntris, st, TICKS_READ() - t0, rspq_wait_ticks - w0, NULL);
    st_tri_ticks += TICKS_READ() - t0;
    st_tri_wait += rspq_wait_ticks - w0;
}
static void draw_tris_inner(const n64_vertex_t *v, uint32_t ntris, const n64_state_t *st) {
    if (!attached || !ntris) return;
    tri_setup_t su = tri_begin(ntris, st);
    n64_vertex_t t[3];
    for (uint32_t k = 0; k < ntris; k++, v += 3) {
        memcpy(t, v, sizeof t);
        tri_one(&su, t);
    }
}

static inline float clamp01(float x) { return x < 0.0f ? 0.0f : x > 1.0f ? 1.0f : x; }

/* render_cpu.rs `vertex` + the depth and colour split of `RenderCpu::tris`. */
static inline void tl_vertex(n64_vertex_t *o, const float *v, float zk, uint32_t blend) {
    o->x = v[0];
    o->y = v[1];
    o->z = clamp01(v[2] * zk);
    o->r = clamp01(v[4]);
    o->g = clamp01(v[5]);
    o->b = clamp01(v[6]);
    o->a = clamp01(v[7]);
    o->s = v[8];
    o->t = v[9];
    o->inv_w = v[3] > 0.0f ? (v[3] < 1.0f ? v[3] : 1.0f) : 1.0f;
    if (blend == N64_BLEND_ADD) {
        float m = o->r > o->g ? o->r : o->g;
        if (o->b > m) m = o->b;
        if (m > 0.0f) {
            o->r /= m;
            o->g /= m;
            o->b /= m;
            o->a *= m;
        } else {
            o->a = 0.0f;
        }
    } else if (blend == N64_BLEND_SUB) {
        float m = o->r > o->g ? o->r : o->g;
        if (o->b > m) m = o->b;
        o->a = m;
        o->r = o->g = o->b = 0.0f;
    }
}

/* rdpq_triangle_rsp (libdragon rdpq_tri.c) for an affine triangle (inv_w = 1) straight from
 * TlVertex floats: the same fixed-point vertex words and commands, without the
 * intermediate float vertex, the 1/w division and the s16.16 conversions of w (both are
 * 1.0 = 0x10000 here). Vertex conversion as tl_vertex + tri_one. */
void __rdpq_autosync_use(uint32_t res);
#define F2_TRI_DATA_LEN 32 /* ROUND_UP((2+1+1+3)*4, 16) */
/* rdpq_tri.c float_to_s16_16 (static there). */
static inline int32_t f2_s16_16(float f) {
    if (f >= 32768.f) return 0x7FFFFFFF;
    if (f < -32768.f) return 0x80000000;
    return floor(f * 65536.f);
}

/* fast_vtx's colour cache (valid within one n64_draw_tl_tris call: same blend mode) and
 * the source vertex of each of the RSP's three triangle-data slots (fast_tri_shared). */
static uint32_t col_key[4], col_rgba;
static bool col_ok;
static const float *slot_src[3];
static inline void fast_vtx(int i, const float *v, const tri_setup_t *su, float zk, uint32_t blend, bool zb, bool tex) {
    int16_t x = floorf(v[0] * 4.0f);
    int16_t y = floorf(v[1] * 4.0f);
    int16_t z = zb ? (int16_t)(clamp01(v[2] * zk) * 0x7FFF) : 0;
    /* Neighbouring vertices mostly share their colour (a shadow or sprite quad): its
     * conversion (four float -> int casts, the additive split's divisions) is reused. */
    const uint32_t *cw = (const uint32_t *)(v + 4);
    uint32_t rgba;
    if (col_ok && cw[0] == col_key[0] && cw[1] == col_key[1] && cw[2] == col_key[2] && cw[3] == col_key[3]) {
        rgba = col_rgba;
    } else {
        float r = clamp01(v[4]), g = clamp01(v[5]), b = clamp01(v[6]), a = clamp01(v[7]);
        if (blend == N64_BLEND_ADD) {
            float m = r > g ? r : g;
            if (b > m) m = b;
            if (m > 0.0f) {
                r /= m;
                g /= m;
                b /= m;
                a *= m;
            } else {
                a = 0.0f;
            }
        } else if (blend == N64_BLEND_SUB) {
            float m = r > g ? r : g;
            if (b > m) m = b;
            a = m;
            r = g = b = 0.0f;
        }
        rgba = ((uint32_t)(r * 255.0f) << 24) | ((uint32_t)(g * 255.0f) << 16) | ((uint32_t)(b * 255.0f) << 8) | (uint32_t)(a * 255.0f);
        col_key[0] = cw[0]; col_key[1] = cw[1]; col_key[2] = cw[2]; col_key[3] = cw[3];
        col_rgba = rgba;
        col_ok = true;
    }
    int16_t s = 0, t = 0;
    int32_t w = 0, inv_w = 0;
    if (tex) {
        float fs = v[8], ft = v[9];
        if (su->mode == 1) {
            fs *= su->sw;
            ft *= su->sh;
        }
        s = fs * 32.0f;
        t = ft * 32.0f;
        /* 1/w as tl_vertex clamps it, w and 1/w as rdpq_triangle_rsp converts them. */
        float iw = v[3] > 0.0f ? (v[3] < 1.0f ? v[3] : 1.0f) : 1.0f;
        if (iw == 1.0f) {
            w = inv_w = 0x10000;
        } else {
            w = f2_s16_16(1.0f / iw);
            inv_w = f2_s16_16(iw);
        }
    }
    rspq_write(RDPQ_OVL_ID, RDPQ_CMD_TRIANGLE_DATA, F2_TRI_DATA_LEN * i, (x << 16) | (y & 0xFFFF), (z << 16), rgba, (s << 16) | (t & 0xFFFF), w, inv_w);
}

/* The fields of a TlVertex that fast_vtx reads (x y z rhw r g b a s t), bitwise. */
static inline bool same_vtx(const float *a, const float *b) {
    const uint32_t *x = (const uint32_t *)a, *y = (const uint32_t *)b;
    for (int k = 0; k < 10; k++)
        if (x[k] != y[k]) return false;
    return true;
}

/* One affine triangle as rdpq_triangle_rsp would send it (fast_vtx), reusing the RSP's
 * triangle-data slots that already hold one of its vertices (a quad's second triangle shares
 * two with its first). The triangle command reads the three slots (saved state of the rdpq
 * overlay, kept across overlay switches) and only sorts pointers to them, so a slot stays
 * valid until it is overwritten, and vertex order does not matter (no culling): the RDP
 * receives the same triangle (its vertices possibly in another order). */
static void fast_tri_shared(const tri_setup_t *su, const float *v, float zk, uint32_t blend) {
    const rdpq_trifmt_t *fmt = su->f;
    bool tex = fmt->tex_offset >= 0, zb = fmt->z_offset >= 0;
    uint32_t res = AUTOSYNC_PIPE;
    if (tex) res |= AUTOSYNC_TILE(fmt->tex_tile) | AUTOSYNC_TMEM(0);
    __rdpq_autosync_use(res);
    uint32_t cmd_id = RDPQ_CMD_TRI | 0x4, rdp_words = 4 + 8;
    if (tex) {
        cmd_id |= 0x2;
        rdp_words += 8;
    }
    if (zb) {
        cmd_id |= 0x1;
        rdp_words += 2;
    }
    const float *nv[3] = {v, v + 12, v + 24};
    bool used[3] = {false, false, false}, have[3] = {false, false, false};
    for (int i = 0; i < 3; i++)
        for (int k = 0; k < 3; k++)
            if (!used[k] && slot_src[k] && same_vtx(slot_src[k], nv[i])) {
                used[k] = have[i] = true;
                break;
            }
    for (int i = 0; i < 3; i++) {
        if (have[i]) continue;
        int k = 0;
        while (used[k]) k++;
        fast_vtx(k, nv[i], su, zk, blend, zb, tex);
        slot_src[k] = nv[i];
        used[k] = true;
        st_fast_vtx++;
    }
    rdpq_write(rdp_words, RDPQ_OVL_ID, RDPQ_CMD_TRIANGLE, 0xC000 | (cmd_id << 8) | (fmt->tex_tile & 7));
    st_rdpq_tris++;
}

void n64_draw_tl_tris_impl(const n64_tl_batch_t *b, const n64_state_t *st) {
    uint32_t t0 = TICKS_READ(), w0 = rspq_wait_ticks;
    if (attached && b->ntris) {
        tri_setup_t su = tri_begin(b->ntris, st);
        const uint8_t *p = b->tris;
        n64_vertex_t t[3];
        col_ok = false;
        slot_src[0] = slot_src[1] = slot_src[2] = NULL;
        for (uint32_t k = 0; k < b->ntris; k++, p += b->stride) {
            const float *v = (const float *)p;
            /* Straight to the RSP command (rdpq_triangle_rsp's words, see fast_vtx). */
            if (su.mode != 2) {
                fast_tri_shared(&su, v, b->zk, st->blend);
                continue;
            }
            /* rdpq_triangle loads all three slots itself. */
            slot_src[0] = slot_src[1] = slot_src[2] = NULL;
            tl_vertex(&t[0], v, b->zk, st->blend);
            tl_vertex(&t[1], v + 12, b->zk, st->blend);
            tl_vertex(&t[2], v + 24, b->zk, st->blend);
            tri_one(&su, t);
        }
    }
    if (TICKS_READ() - t0 > TICKS_FROM_MS(2)) slow_batch("triangles", b->ntris, st, TICKS_READ() - t0, rspq_wait_ticks - w0, NULL);
    st_tri_ticks += TICKS_READ() - t0;
    st_tri_wait += rspq_wait_ticks - w0;
}

static void draw_rects_inner(const n64_rect_t *r, uint32_t n, const n64_state_t *st);
static uint32_t st_rect_path[16];

/* A CPU batch that took over 2 ms (spike hunting): what it drew. */
static void slow_batch(const char *what, uint32_t n, const n64_state_t *st, uint32_t ticks, uint32_t wait, const uint32_t *paths0) {
    const tex_t *t = st->tex && st->tex < MAX_TEXTURES && textures[st->tex].used ? &textures[st->tex] : NULL;
    debugf("slow cpu batch: %lu %s, %lu us (rspq wait %lu us), tex %lu", (unsigned long)n, what, (unsigned long)TICKS_TO_US(ticks), (unsigned long)TICKS_TO_US(wait), (unsigned long)st->tex);
    if (t)
        debugf(" (%dx%d fmt %d, %d chunk(s), small %dx%d%s%s%s)", t->width, t->full[0].height, (int)surface_get_format(&t->full[0]), t->nfull,
            t->small.width, t->small.height, t->per_tile ? ", per-tile palettes" : "", t->has_addv ? ", additive variant" : "", t->tile3d ? ", tiled" : "");
    debugf(" blend %lu flags %lx", (unsigned long)st->blend, (unsigned long)st->flags);
    if (paths0) {
        debugf(", paths");
        for (int i = 0; i < 16; i++)
            if (st_rect_path[i] != paths0[i]) debugf(" %d:%lu", i, (unsigned long)(st_rect_path[i] - paths0[i]));
    }
    debugf("\n");
}

void n64_draw_rects_impl(const n64_rect_t *r, uint32_t n, const n64_state_t *st) {
    uint32_t t0 = TICKS_READ(), w0 = rspq_wait_ticks;
    uint32_t paths0[16];
    memcpy(paths0, st_rect_path, sizeof paths0);
    st_rects += n;
    draw_rects_inner(r, n, st);
    uint32_t dt = TICKS_READ() - t0;
    st_rect_ticks += dt;
    st_rect_wait += rspq_wait_ticks - w0;
    if (dt > TICKS_FROM_MS(2)) slow_batch("rects", n, st, dt, rspq_wait_ticks - w0, paths0);
}

/* Draws [x0,x1)x[y0,y1) from texels s0..s1 / t0..t1 of `s` (s1 < s0 / t1 < t0 = flipped)
 * when `s` fits TMEM; false = use the blit path. */
static bool rect_fast(const tex_t *tx, const surface_t *s, float x0, float y0, float x1, float y1, float s0, float t0, float s1, float t1) {
    if (!fits_tmem_block(s)) return false;
    if (rect_tmem != s) {
        use_tlut(tx, s, 0);
        tex_upload_cached(TILE0, s, false, 0, 0, s->width, s->height);
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
    tex_upload_cached(TILE0, s, false, u0, v0, u1, v1);
    rdpq_texture_rectangle_scaled(TILE0, r->x0, r->y0, r->x1, r->y1, fx ? is1 : is0, fy ? it1 : it0, fx ? is0 : is1, fy ? it0 : it1);
    return true;
}

/* One rect of draw_rects_inner; returns the path taken (statistics). */
static int draw_one_rect(const n64_rect_t *r, const n64_state_t *st, bool tex, const tex_t *tx) {
    if (!tex) {
        rdpq_fill_rectangle(r->x0, r->y0, r->x1, r->y1);
        return 1;
    }
    const surface_t *s = &tx->full[0];
    float w = tx->width, h = s->height;
    if (st->blend == N64_BLEND_ADD) {
        /* Additive: the alpha-blended variant (see make_add_variant), one TMEM-sized image. */
        const surface_t *a = n64_tex_add_surface(st->tex);
        if (!a) return 2;
        float s0 = r->s0 * a->width, s1 = r->s1 * a->width, t0 = r->t0 * a->height, t1 = r->t1 * a->height;
        if (rect_fast(tx, a, r->x0, r->y0, r->x1, r->y1, s0, t0, s1, t1)) return 9;
        rect_tmem = NULL;
        bool fx = s1 < s0, fy = t1 < t0;
        float sa = fx ? s1 : s0, sb = fx ? s0 : s1, ta = fy ? t1 : t0, tb = fy ? t0 : t1;
        int is0 = (int)(sa + 0.5f), it0 = (int)(ta + 0.5f), is1 = (int)(sb + 0.5f), it1 = (int)(tb + 0.5f);
        if (is1 <= is0 || it1 <= it0) return 2;
        use_tlut(tx, a, 0);
        rdpq_tex_blit(a, r->x0, r->y0, &(rdpq_blitparms_t){
            .s0 = is0, .t0 = it0, .width = is1 - is0, .height = it1 - it0, .flip_x = fx, .flip_y = fy,
            .scale_x = (r->x1 - r->x0) / (is1 - is0), .scale_y = (r->y1 - r->y0) / (it1 - it0),
            .filtering = (st->flags & N64_BILINEAR) != 0,
        });
        return 0;
    }
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
     * the TMEM-sized box-filtered copy has about one texel per pixel, so blit it instead
     * (one texture load instead of one per TMEM-sized piece; phase 3P). Up to 1.25x
     * magnification of the copy is allowed: the HUD icons (64x64 / 128x128 RGBA32, copies
     * of 32x32) are drawn 32-35 px wide and otherwise took the multi-load blit (~0.45 ms
     * each). */
    if (tx->nfull == 1 && !tx->per_tile && !fits_tmem_block(s)) {
        const surface_t *tm = n64_tex_tmem_surface(st->tex);
        if (tm && tm != s && tm->width > 0 && tm->height > 0) {
            float kx = (float)tm->width / s->width, ky = (float)tm->height / s->height;
            int a0 = (int)(is0 * kx), b0 = (int)(it0 * ky), a1 = (int)(is1 * kx + 0.5f), b1 = (int)(it1 * ky + 0.5f);
            if (a1 > a0 && b1 > b0 && r->x1 - r->x0 <= (a1 - a0) * 1.25f && r->y1 - r->y0 <= (b1 - b0) * 1.25f) {
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
    bool zt = (st->flags & N64_ZTEST) != 0;
    /* Primitive colour and depth only when they change within the batch (the coins of a
     * row share both). */
    uint32_t last_rgba = 0;
    int32_t last_z = -1;
    for (uint32_t k = 0; k < n; k++, r++) {
        if (zt) {
            float z = r->z < 0.0f ? 0.0f : r->z > 1.0f ? 1.0f : r->z;
            int32_t zi = (uint16_t)(z * 0x7FFF);
            if (zi != last_z) rdpq_set_prim_depth_raw((uint16_t)zi, 0);
            last_z = zi;
        }
        if (k == 0 || r->rgba != last_rgba) rdpq_set_prim_color(color_from_packed32(r->rgba));
        last_rgba = r->rgba;
        int path = draw_one_rect(r, st, tex, tx);
        st_rect_path[path & 15]++;
    }
}


/* ---- frame limit (30 / 20 fps settings): a minimum interval between flips ----
 * libdragon's display_set_fps_limit shows frames only on every 2nd (30 fps) / 3rd (20 fps)
 * vblank at a fixed phase: a frame ready 1 ms after such a vblank waits for the next one,
 * so the previous image stays 66 ms (an instant 15 fps) for a frame that took 34 ms.
 * Instead the display shows every vblank (limit 0) and a finished frame is handed to
 * display_show no earlier than its turn: at least `pace_vblanks` vblanks after the last
 * flip, else at the first vblank where it may flip; a late frame then costs one vblank
 * (50 ms), and the cadence re-forms behind it. The CPU still runs up to the same number of
 * framebuffers ahead (display_get waits for a free one). F2N64_CFLAGS=-DF2N64_FLIP_LIBDRAGON
 * (build time) keeps display_set_fps_limit. */
#ifndef F2N64_FLIP_LIBDRAGON
#define PACED_FLIPS 1
#else
#define PACED_FLIPS 0
#endif
static volatile uint32_t vb_count, flip_vb, pace_vblanks;
static surface_t *volatile pace_pend[3];
static volatile int pace_npend;
/* Display statistics: vblanks between two changes of the shown framebuffer. */
static volatile uint32_t vb_origin, vb_since_flip, flip_hist[6];

void rdpq_detach_cb(void (*cb)(void *), void *arg);
static void pace_show(surface_t *s) {
    display_show(s);
    flip_vb = vb_count + 1; /* (display's own vblank handler has run: visible at the next) */
}

/* RDP finished a frame (interrupt context). */
static void pace_rdp_done(void *arg) {
    if (pace_npend == 0 && vb_count + 1 >= flip_vb + pace_vblanks) pace_show((surface_t *)arg);
    else if (pace_npend < 3) pace_pend[pace_npend++] = (surface_t *)arg;
    else pace_show((surface_t *)arg); /* cannot happen with 3 framebuffers */
}

/* Installed after display_init: runs after libdragon's display handler in each vblank. */
static void pace_vblank(void *arg) {
    vb_count++;
    if (pace_npend && vb_count + 1 >= flip_vb + pace_vblanks) {
        pace_show(pace_pend[0]);
        pace_pend[0] = pace_pend[1];
        pace_pend[1] = pace_pend[2];
        pace_npend--;
    }
    uint32_t o = *VI_ORIGIN;
    vb_since_flip++;
    if (o != vb_origin) {
        vb_origin = o;
        flip_hist[vb_since_flip < 5 ? vb_since_flip : 5]++;
        vb_since_flip = 0;
    }
}

/* Flips since the last call by the number of vblanks the previous image was shown (index
 * 1..4, 5 = 5 or more); resets. */
void n64_flip_stats_impl(uint32_t *out) {
    disable_interrupts();
    for (int i = 0; i < 6; i++) {
        out[i] = flip_hist[i];
        flip_hist[i] = 0;
    }
    enable_interrupts();
}

void n64_present_impl(void) {
    upc_frame++;
    if (upc_nretired) upc_collect();
    if (++st_frames == 300) {
        debugf("cputri: per frame %lu calls, %lu source tris (%lu tiled), %lu rdpq triangles (%lu vertices converted), %lu tile uploads, tris %lu us (wait %lu); %lu rects %lu us (wait %lu); all rspq waits %lu us; apply_state %lu us (%lu mode resets, tri tex uploads %lu us)\n", st_calls / 300, st_src_tris / 300, st_tiled_src / 300, st_rdpq_tris / 300, st_fast_vtx / 300, st_tiled_uploads / 300, TICKS_TO_US(st_tri_ticks) / 300, TICKS_TO_US(st_tri_wait) / 300, st_rects / 300, TICKS_TO_US(st_rect_ticks) / 300, TICKS_TO_US(st_rect_wait) / 300, TICKS_TO_US(rspq_wait_ticks - st_wait0) / 300, TICKS_TO_US(st_mode_ticks) / 300, st_modes / 300, TICKS_TO_US(st_tex_ticks) / 300);
        debugf("cputri: rect paths (sum over 300 frames; draw_one_rect return codes):");
        for (int i = 0; i < 16; i++) if (st_rect_path[i]) debugf(" %d:%lu", i, (unsigned long)st_rect_path[i]);
        debugf("; texture upload cache per frame: %lu hits, %lu misses (%lu uploaded directly)\n", st_upc_hits / 300, st_upc_misses / 300, st_upc_direct / 300);
        st_upc_hits = st_upc_misses = st_upc_direct = 0;
        memset(st_rect_path, 0, sizeof st_rect_path);
        st_mode_ticks = st_modes = st_tex_ticks = 0;
        st_tri_wait = st_rect_wait = 0; st_wait0 = rspq_wait_ticks;
        st_tri_ticks = st_rect_ticks = st_rects = 0;
        st_frames = st_calls = st_src_tris = st_rdpq_tris = st_tiled_src = st_tiled_uploads = st_fast_vtx = 0;
    }
    if (!attached) return;
    if (pace_vblanks) rdpq_detach_cb(pace_rdp_done, last_fb);
    else rdpq_detach_show();
    attached = false;
}

void n64_rdp_wait_impl(void) { rspq_wait(); }

void n64_set_fps_limit_impl(uint32_t fps) {
    fps_limit = fps;
    if (PACED_FLIPS) {
        float hz = display_get_refresh_rate();
        pace_vblanks = fps ? (uint32_t)(hz / fps + 0.5f) : 0;
        display_set_fps_limit(0);
    } else {
        display_set_fps_limit(fps);
    }
}

/* ---- sampling profiler (feature `profile`): a 2 kHz timer interrupt records the
 * interrupted PC (and $ra) of the main thread in 32-byte buckets; n64_prof(1) prints the
 * hottest buckets (map them to functions with mips64-elf-addr2line -f -e build/frogger2.elf)
 * and resets. ---- */
#define PROF_BASE 0x80000000u
#define PROF_SHIFT 5
#define PROF_BUCKETS (0x200000u >> PROF_SHIFT) /* first 2 MiB of RDRAM: the code */
extern void *interrupt_exception_frame;
static uint16_t *prof_pc, *prof_ra, *prof_rm;
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
    /* Callers of memcpy/memset (leaf functions: $ra is the call site). */
    if (j < PROF_BUCKETS && ((epc - (uint32_t)&memcpy) < 0x400 || (epc - (uint32_t)&memset) < 0x200) && prof_rm[j] < 0xffff) prof_rm[j]++;
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
        prof_rm = calloc(PROF_BUCKETS, 2);
        timer_init();
        prof_timer = new_timer(TIMER_TICKS(500), TF_CONTINUOUS, prof_cb);
    } else if (cmd == 1 && prof_pc) {
        stop_timer(prof_timer);
        debugf("PROF samples %lu (outside code %lu)\n", (unsigned long)prof_samples, (unsigned long)prof_other);
        prof_top("pc", prof_pc);
        prof_top("ra", prof_ra);
        prof_top("rm", prof_rm);
        memset(prof_rm, 0, PROF_BUCKETS * 2);
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

#ifdef F2N64_CODEPAD
/* Measurement: F2N64_CFLAGS=-DF2N64_CODEPAD=N bytes of padding at the end of the hot flush
 * region, to sample code layouts (everything after it moves). */
__attribute__((section(".text.f2hot_f"), noinline)) void f2_layout_pad(void) { __asm__ volatile(".space %0" :: "i"(F2N64_CODEPAD)); }
#endif
int main(void) {
#ifdef F2N64_CODEPAD
    if (get_memory_size() == 1) f2_layout_pad();
#endif
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
#ifndef F2N64_AUDIO_RATE
#define F2N64_AUDIO_RATE 32000 /* measurement: F2N64_CFLAGS=-DF2N64_AUDIO_RATE=22050 */
#endif
    audio_init(F2N64_AUDIO_RATE, AUDIO_INIT_LATENCY_MS(300));
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
    vi_install_vblank_handler(pace_vblank, NULL);

    /* After the boot-time allocations (framebuffers, z-buffer, audio, RSP queues), which live
     * for the whole run, so the pool holds only per-level and transient large blocks. */
    big_init();
    floor_selftest();
#ifdef F2N64_MEMBENCH
    {
        /* Memory access costs in this emulator / console (cycles per access). */
        static uint32_t cbuf[4096] __attribute__((aligned(16)));
        volatile uint32_t *ub = (volatile uint32_t *)UncachedAddr(malloc_uncached(16384));
        uint32_t t0, sum = 0;
        t0 = TICKS_READ(); for (int i = 0; i < 4096; i++) ub[i] = i; uint32_t a = TICKS_READ() - t0;
        t0 = TICKS_READ(); for (int i = 0; i < 4096; i++) sum += ub[i]; uint32_t b = TICKS_READ() - t0;
        t0 = TICKS_READ(); for (int i = 0; i < 4096; i++) cbuf[i] = i; uint32_t c = TICKS_READ() - t0;
        t0 = TICKS_READ(); for (int i = 0; i < 4096; i++) cbuf[i] = i; uint32_t d = TICKS_READ() - t0;
        t0 = TICKS_READ(); for (int i = 0; i < 4096; i++) sum += cbuf[i]; uint32_t e = TICKS_READ() - t0;
        volatile uint64_t *ub64 = (volatile uint64_t *)ub;
        t0 = TICKS_READ(); for (int i = 0; i < 2048; i++) ub64[i] = i; uint32_t f = TICKS_READ() - t0;
        t0 = TICKS_READ(); for (int i = 0; i < 1024; i++) rdpq_set_prim_color(RGBA32(i, 0, 0, 255)); rspq_wait(); uint32_t g = TICKS_READ() - t0;
        /* COP0 Count runs at half the CPU clock: x2 for CPU cycles. */
        debugf("membench (CPU cycles per op): uncached store %lu, uncached load %lu, cached store (cold) %lu, (warm) %lu, cached load %lu, uncached 64-bit store %lu, rdpq_set_prim_color %lu [sum %lu]\n",
               a * 2 / 4096, b * 2 / 4096, c * 2 / 4096, d * 2 / 4096, e * 2 / 4096, f * 2 / 2048, g * 2 / 1024, sum);
    }
#endif
#ifdef F2N64_AUDIO_PROF
    /* libdragon's mixer profile slots (PROFILE_SCOPE is compiled in): dumped every 5 s. */
    extern void __mixer_profile_init(void);
    profile_init(&(profile_parms_t){.num_slots = 64, .dump_stderr_interval = 5.0f});
    __mixer_profile_init();
#endif
    debugf("level heap %lu KiB, main heap reserve %d KiB\n", (unsigned long)(pool_size / 1024), F2N64_MAIN_RESERVE_KIB);
    f2n64_call(f2n64_init);
    for (;;) {
        joypad_poll();
        n64_audio_poll_impl();
        f2n64_call(f2n64_frame);
        n64_audio_poll_impl();
#ifdef F2N64_AUDIO_PROF
        profile_next_frame();
#endif
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
