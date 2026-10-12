/* r3d: the RSP 3D renderer of the Frogger 2 N64 port (phase 2R).
 *
 * Geometry runs on the RSP through libdragon's magma pipeline with our own vertex shader
 * (rsp_f2mdx.S: the MDX projection, screen/near/far face rejection) and clipping overlay
 * (rsp_f2clip.S). Meshes come from F2NM files (crates/f2-n64fmt, docs/n64-model-format.md):
 * each texture group of a mesh is recorded once into an rspq block (bind vertex buffer,
 * vertex-cache loads, triangles), so a draw costs the CPU one inline uniform (matrix +
 * colour) plus, per group, a texture upload when the texture changes and a block call.
 * Render states (frame lists) are set with rdpq modes. Rust (src/r3d.rs) computes the
 * matrices and owns the frame lists; interface in n64_ffi.h. */
#include <libdragon.h>
#include <magma.h>
#include <string.h>
#include <malloc.h>
#include "n64_ffi.h"
#include "r3d_rsp_syms.h"

DEFINE_RSP_UCODE(rsp_f2mdx);
DEFINE_RSP_UCODE(rsp_f2clip);
DEFINE_RSP_UCODE(rsp_f2modge);

#define F2_ATTR_POS 0
#define F2_ATTR_COLOR 1
#define F2_ATTR_ST 2
#define F2_BIND_DRAW 0
#define F2_BIND_TEX 1
#define F2_BIND_FRAME 2

#define R3D_ROUND_UP(n, d) (((n) + (d) - 1) / (d) * (d))
#define MAX_R3D_TEX 1024
#define MAX_R3D_GROUPS 8192

typedef struct {
    surface_t surf;
    uint16_t *tlut;
    uint16_t tlut_count;
    uint16_t width;
    /* Load descriptors for the shader's texture command (F2_LoadTex in rsp_f2mdx.S), per
     * variant (wrap | bilinear << 1), built on first use. */
    struct r3d_texdesc_s *desc[4];
    uint16_t src;       /* main.c texture this aliases (0 = F2NM file texture) */
    uint16_t addv;      /* r3d texture of its additive variant (0 = not made yet) */
} r3d_tex_t;

/* F2_LoadTex descriptor (F2_TEXDESC_SIZE = 144 bytes, 16-aligned). */
typedef struct r3d_texdesc_s {
    uint16_t rdp_bytes;
    uint16_t pad[3];
    int16_t u[12];          /* F2_TEX uniform */
    uint64_t cmds[14];
} __attribute__((aligned(16))) r3d_texdesc_t;
_Static_assert(sizeof(r3d_texdesc_t) == 144, "F2_TEXDESC_SIZE");

typedef struct {
    rspq_block_t *block;     /* rigid groups: recorded geometry */
    const void *verts;       /* skinned groups: replayed every draw */
    const uint32_t *cmds;
    uint32_t n_cmds;
} r3d_group_t;

/* F2_TEX uniform (24 bytes). */
typedef struct {
    int16_t i[4];
    uint16_t f[4];
    int16_t o[4];
} __attribute__((aligned(8))) r3d_tex_uniform_t;

static mg_pipeline_t *pipe;
static const mg_uniform_t *u_draw, *u_tex, *u_frame;
static uint32_t clip_text, clip_size;
static uint32_t modge_text, modge_size;
static r3d_tex_t texs[MAX_R3D_TEX];
static uint32_t num_texs = 1;  /* 0 = none */
static r3d_group_t groups[MAX_R3D_GROUPS];
static uint32_t num_groups = 1;
static uint32_t cur_tex;       /* texture currently in TMEM (+ its F2_TEX uniform) */
static uint32_t cur_tex_flags; /* wrap/bilinear it was uploaded with */
static uint32_t cur_list_flags;
static int cur_tlut = -1;      /* SOM TLUT mode currently set (-1 = unknown) */
static n64_r3d_stats_t stats;
/* Measurement modes (main.c probes): while an outer probe block is being recorded no new
 * block can be recorded; in flat mode groups and textures are emitted inline instead of
 * through their blocks, so a whole frame can be recorded and replayed without nested
 * blocks (replaying magma blocks nested in an rdpq-recorded block corrupts the RDP
 * buffers in this libdragon preview). */
bool r3d_probe_recording;
static bool r3d_flat;

void r3d_set_flat(bool on) { r3d_flat = on; }

/* RDRAM address and DMA size of a ucode's magma overlay text (from _MG_OVERLAY on, like
 * magma.c's get_overlay_span; offsets generated from the ucode ELFs by the Makefile). */
static void get_overlay_text(rsp_ucode_t *ucode, uint32_t ovl, uint32_t *code, uint32_t *size) {
    uint32_t total = (uint8_t *)ucode->code_end - ucode->code;
    *code = PhysicalAddr(ucode->code + ovl);
    *size = R3D_ROUND_UP(total - ovl, 8) - 1;
}

void n64_r3d_init_impl(void) {
    mg_init();
    mg_vertex_attribute_t attrs[] = {
        {.input = F2_ATTR_POS, .offset = 0},
        {.input = F2_ATTR_COLOR, .offset = 8},
        {.input = F2_ATTR_ST, .offset = 12},
    };
    pipe = mg_pipeline_create(&(mg_pipeline_parms_t){
        .vertex_shader_ucode = &rsp_f2mdx,
        .vertex_layout = {.stride = 16, .attribute_count = 3, .attributes = attrs},
    });
    u_draw = mg_pipeline_get_uniform(pipe, F2_BIND_DRAW);
    u_tex = mg_pipeline_get_uniform(pipe, F2_BIND_TEX);
    u_frame = mg_pipeline_get_uniform(pipe, F2_BIND_FRAME);
    assertf(u_draw->size == sizeof(n64_r3d_draw_uniform_t), "F2_DRAW size %lu", u_draw->size);
    assertf(u_tex->size == sizeof(r3d_tex_uniform_t), "F2_TEX size %lu", u_tex->size);
    assertf(u_frame->size == offsetof(n64_r3d_frame_t, guard), "F2_FRAME size %lu", u_frame->size);
    assertf(RSP_F2MDX_MG_OVERLAY_OFFSET == RSP_F2CLIP_MG_OVERLAY_OFFSET, "magma core text differs between ucodes");
    get_overlay_text(&rsp_f2clip, RSP_F2CLIP_MG_OVERLAY_OFFSET, &clip_text, &clip_size);
    assertf(RSP_F2MDX_MG_OVERLAY_OFFSET == RSP_F2MODGE_MG_OVERLAY_OFFSET, "magma core text differs (modge overlay)");
    assertf(RSP_F2MODGE_ENTRY_OFFSET == 16, "F2_ModgeEntry at overlay +%d", RSP_F2MODGE_ENTRY_OFFSET);
    get_overlay_text(&rsp_f2modge, RSP_F2MODGE_MG_OVERLAY_OFFSET, &modge_text, &modge_size);
    debugf("r3d: magma pipeline up (overlay at IMEM +0x%x, shader %lu bytes, clip overlay %lu bytes; F2_DRAW @%lu F2_TEX @%lu F2_FRAME @%lu)\n",
           RSP_F2MDX_MG_OVERLAY_OFFSET, (uint32_t)((uint8_t *)rsp_f2mdx.code_end - rsp_f2mdx.code) - RSP_F2MDX_MG_OVERLAY_OFFSET,
           clip_size + 1, u_draw->offset, u_tex->offset, u_frame->offset);
}

uint32_t n64_r3d_tex_create_impl(const n64_r3d_tex_desc_t *d) {
    static const tex_format_t fmts[8] = {FMT_RGBA16, FMT_CI8, FMT_CI4, FMT_IA8, FMT_IA16, FMT_I8, FMT_I4, FMT_RGBA32};
    if (num_texs >= MAX_R3D_TEX || d->fmt > 7) return 0;
    tex_format_t fmt = fmts[d->fmt];
    uint32_t stride = TEX_FORMAT_PIX2BYTES(fmt, d->width);
    data_cache_hit_writeback(d->texels, stride * d->height);
    r3d_tex_t *t = &texs[num_texs];
    t->surf = surface_make((void *)d->texels, fmt, d->width, d->height, stride);
    t->tlut = (uint16_t *)d->tlut;
    t->tlut_count = d->tlut_count;
    t->width = d->width;
    if (d->tlut) data_cache_hit_writeback(d->tlut, d->tlut_count * 2);
    return num_texs++;
}

/* An r3d texture sampling the TMEM-sized surface of main.c texture `tex` (RGBA16/RGBA32,
 * power-of-two sizes for wrapping); 0 if it cannot be used. */
const surface_t *n64_tex_tmem_surface(uint32_t h);
const uint16_t *n64_tex_tlut(uint32_t h, uint32_t *count);
uint32_t n64_r3d_tex_alias_impl(uint32_t tex) {
    const surface_t *s = n64_tex_tmem_surface(tex);
    if (!s || num_texs >= MAX_R3D_TEX) return 0;
    tex_format_t f = surface_get_format(s);
    uint32_t limit = (f == FMT_CI4 || f == FMT_CI8) ? 2048 : 4096; /* RGBA32: 2 x 2 KB halves */
    uint32_t tlut_count = 0;
    const uint16_t *tlut = n64_tex_tlut(tex, &tlut_count);
    const char *why = NULL;
    if ((s->width & (s->width - 1)) || (s->height & (s->height - 1))) why = "not a power of two";
    else if (s->stride != TEX_FORMAT_PIX2BYTES(f, s->width) || s->stride < 8) why = "row stride";
    /* LOAD_BLOCK: the whole texture in one load (CI/RGBA32: 2 KB of texels). */
    else if (s->stride * s->height > limit) why = "larger than TMEM";
    else if ((f == FMT_CI4 || f == FMT_CI8) && !tlut) why = "no palette";
    if (why) {
        debugf("r3d: texture %lu (%dx%d %s, stride %d) not usable: %s\n", tex, s->width, s->height, tex_format_name(f), s->stride, why);
        return 0;
    }
    r3d_tex_t *t = &texs[num_texs];
    memset(t, 0, sizeof *t);
    t->surf = *s;
    t->width = s->width;
    t->src = tex;
    if (f == FMT_CI4 || f == FMT_CI8) {
        t->tlut = (uint16_t *)tlut;
        t->tlut_count = f == FMT_CI4 ? 16 : (tlut_count > 256 ? 256 : tlut_count);
    }
    return num_texs++;
}

/* The additive variant of r3d texture h (main.c make_add_variant: colour normalised to its
 * brightest channel, alpha = alpha x brightness, so the alpha blender adds exactly the D3D
 * additive term without the RDP's wrap-around), made on first use; h itself if impossible. */
const surface_t *n64_tex_add_surface(uint32_t h);
static uint32_t additive_tex(uint32_t h) {
    r3d_tex_t *t = &texs[h];
    if (t->addv) return t->addv;
    const surface_t *s = t->src ? n64_tex_add_surface(t->src) : NULL;
    if (!s || num_texs >= MAX_R3D_TEX) {
        debugf("r3d: no additive variant for texture %lu (src %u)\n", h, t->src);
        return h;
    }
    r3d_tex_t *a = &texs[num_texs];
    memset(a, 0, sizeof *a);
    a->surf = *s;
    a->width = s->width;
    t->addv = num_texs;
    uint32_t acc[4] = {0, 0, 0, 0};
    for (int y = 0; y < s->height; y++)
        for (int x = 0; x < s->width; x++)
            for (int k = 0; k < 4; k++) acc[k] += ((uint8_t *)s->buffer)[y * s->stride + x * 4 + k];
    uint32_t n = s->width * s->height;
    debugf("r3d: additive variant of texture %lu (src %u, %dx%d %s) -> %lu (%dx%d), mean rgba %lu %lu %lu %lu\n", h, t->src, t->surf.width,
           t->surf.height, tex_format_name(surface_get_format(&t->surf)), num_texs, s->width, s->height, acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n);
    return num_texs++;
}

/* Frees textures >= tex_first and groups >= group_first (level unload). */
void n64_r3d_reset_impl(uint32_t tex_first, uint32_t group_first) {
    rspq_wait();
    if (tex_first < 1) tex_first = 1;
    if (group_first < 1) group_first = 1;
    for (uint32_t i = tex_first; i < num_texs; i++) {
        for (int v = 0; v < 4; v++)
            if (texs[i].desc[v]) free(texs[i].desc[v]);
        memset(&texs[i], 0, sizeof texs[i]);
    }
    if (tex_first < num_texs) num_texs = tex_first;
    for (uint32_t i = group_first; i < num_groups; i++) {
        if (groups[i].block) rspq_block_free(groups[i].block);
        memset(&groups[i], 0, sizeof groups[i]);
    }
    if (group_first < num_groups) num_groups = group_first;
    cur_tex = 0;
}

/* Current counts (next texture handle << 16 | next group handle). */
uint32_t n64_r3d_counts_impl(void) { return (num_texs << 16) | num_groups; }

/* Queues F2_Modge (rsp_f2mdx.S): a 0-vertex load whose cache offset holds the descriptor size. */
void n64_r3d_modge_impl(const void *desc, uint32_t size) {
    mg_cmd_write(MG_CMD_LOAD_VERTICES, PhysicalAddr(desc), size << 16);
}

uint32_t n64_r3d_group_create_impl(const n64_r3d_group_desc_t *d) {
    if (num_groups >= MAX_R3D_GROUPS) return 0;
    r3d_group_t *g = &groups[num_groups];
    data_cache_hit_writeback(d->verts, d->n_verts * 16);
    g->verts = d->verts;
    g->cmds = d->cmds;
    g->n_cmds = d->n_cmds;
    g->block = NULL;
    if (!d->skinned) {
        rspq_block_begin();
        mg_bind_vertex_buffer(d->verts);
        mg_draw_begin();
        for (uint32_t i = 0; i < d->n_cmds; i++) {
            uint32_t w = d->cmds[i];
            switch (w >> 28) {
            case 1: mg_load_vertices((w >> 12) & 0xFFFF, w & 63, (w >> 6) & 63); break;
            case 2: mg_draw_triangle((w >> 16) & 0xFF, (w >> 8) & 0xFF, w & 0xFF); break;
            default: break;
            }
        }
        mg_draw_end();
        g->block = rspq_block_end();
    }
    return num_groups++;
}

/* RDP render state of a frame list (renderer.rs build() / soft.rs semantics). */
static void apply_list(const n64_r3d_list_t *st) {
    rdpq_mode_begin();
    rdpq_set_mode_standard();
    rdpq_mode_combiner(RDPQ_COMBINER_TEX_SHADE);
    rdpq_mode_zbuf(st->flags & N64_ZTEST, st->flags & N64_ZWRITE);
    rdpq_mode_filter(st->flags & N64_BILINEAR ? FILTER_BILINEAR : FILTER_POINT);
    rdpq_mode_persp(false);
    rdpq_mode_dithering(DITHER_SQUARE_SQUARE);
    /* tightAlphaCmpRS (> 100) = threshold 101; normalAlphaCmpRS (!= 0) = 1. */
    rdpq_mode_alphacompare((int)st->alpha_ref);
    switch (st->blend) {
    case N64_BLEND_ADD:
        /* xluAddRS (SRCALPHA, ONE) has no non-overflowing RDP equivalent (the blender
         * wraps instead of clamping): src*a + dst*(1-a) with each texture's additive
         * variant (additive_tex: the src*a term is exact, the background is attenuated
         * only where the effect is bright), unless the true (wrapping) formula is asked. */
        rdpq_mode_blender(st->flags & N64_TRUE_ADDITIVE ? RDPQ_BLENDER_ADDITIVE : RDPQ_BLENDER_MULTIPLY);
        break;
    case N64_BLEND_SUB:
        /* xluSubRS (ZERO, INVSRCCOLOR): dst * (1 - src). Approximated with the source
         * alpha as the (grey) factor: dst * (1 - a). */
        rdpq_mode_blender(RDPQ_BLENDER((BLEND_RGB, IN_ALPHA, MEMORY_RGB, INV_MUX_ALPHA)));
        break;
    default: rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY); break;
    }
    if (st->flags & N64_FOG) {
        rdpq_mode_fog(RDPQ_FOG_STANDARD);
    }
    rdpq_mode_end();
    cur_tlut = 0;
    if (st->blend == N64_BLEND_SUB) rdpq_set_blend_color(RGBA32(0, 0, 0, 255));
    mg_set_culling(&(mg_culling_parms_t){
        .cull_mode = st->cull ? MG_CULL_MODE_BACK : MG_CULL_MODE_NONE,
        .front_face = (st->cull & 2) ? MG_FRONT_FACE_COUNTER_CLOCKWISE : MG_FRONT_FACE_CLOCKWISE,
    });
}

/* RDP command encodings (texture loading). */
#define RDP_SET_TEX_IMAGE(fmt, size, width, addr) \
    ((0x3DULL << 56) | ((uint64_t)(fmt) << 53) | ((uint64_t)(size) << 51) | ((uint64_t)((width) - 1) << 32) | ((addr) & 0x3FFFFFF))
#define RDP_SET_TILE(fmt, size, line, tmem, tile, pal, ct, mt, maskt, cs, ms, masks) \
    ((0x35ULL << 56) | ((uint64_t)(fmt) << 53) | ((uint64_t)(size) << 51) | ((uint64_t)(line) << 41) | ((uint64_t)(tmem) << 32) | \
     ((uint64_t)(tile) << 24) | ((uint64_t)(pal) << 20) | ((uint64_t)(ct) << 19) | ((uint64_t)(mt) << 18) | ((uint64_t)(maskt) << 14) | \
     ((uint64_t)(cs) << 9) | ((uint64_t)(ms) << 8) | ((uint64_t)(masks) << 4))
#define RDP_LOAD_BLOCK(tile, sh, dxt) ((0x33ULL << 56) | ((uint64_t)(tile) << 24) | ((uint64_t)(sh) << 12) | (dxt))
#define RDP_LOAD_TLUT(tile, n) ((0x30ULL << 56) | ((uint64_t)(tile) << 24) | ((uint64_t)(((n) - 1) << 2) << 12))
#define RDP_SET_TILE_SIZE(tile, w, h) ((0x32ULL << 56) | ((uint64_t)(tile) << 24) | ((uint64_t)(((w) - 1) << 2) << 12) | (((h) - 1) << 2))
#define RDP_SYNC_LOAD (0x31ULL << 56)
#define RDP_SYNC_PIPE (0x27ULL << 56)
#define RDP_SYNC_TILE (0x28ULL << 56)

static uint32_t log2u(uint32_t v) { uint32_t l = 0; while ((1u << l) < v) l++; return l; }

static void tex_uniform(const r3d_tex_t *t, uint32_t list_flags, int16_t u[12]) {
    /* st (1/1024 of the texture) -> s10.5 texels: x width/32. Bilinear on the RDP centres
     * texels on integer coordinates, D3D on +0.5: shift by half a texel. */
    uint32_t sw = t->surf.width, sh = t->surf.height;
    int16_t o = list_flags & N64_BILINEAR ? -16 : 0;
    int16_t v[12] = {(int16_t)(sw >> 5), (int16_t)(sh >> 5), 0, 0, (int16_t)((sw & 31) << 11), (int16_t)((sh & 31) << 11), 0, 0, o, o, 0, 0};
    memcpy(u, v, sizeof v);
}

/* Builds the RDP load sequence (libultra gsDPLoadTLUT / gsDPLoadTextureBlock order). */
static r3d_texdesc_t *build_desc(const r3d_tex_t *t, uint32_t list_flags) {
    r3d_texdesc_t *d = memalign(16, sizeof *d);
    memset(d, 0, sizeof *d);
    tex_uniform(t, list_flags, d->u);
    tex_format_t f = surface_get_format(&t->surf);
    uint32_t rfmt = f >> 2, rsize = f & 3; /* _RDP_FORMAT_CODE(fmt, size) */
    uint32_t w = t->surf.width, h = t->surf.height, n = 0;
    bool wrap = list_flags & N64_WRAP;
    uint32_t masks = wrap ? log2u(w) : 0, maskt = wrap ? log2u(h) : 0;
    uint32_t clamp = wrap ? 0 : 1;
    d->cmds[n++] = RDP_SYNC_TILE;
    if (t->tlut) {
        d->cmds[n++] = RDP_SET_TEX_IMAGE(0, 2, 1, PhysicalAddr(t->tlut));
        d->cmds[n++] = RDP_SET_TILE(0, 0, 0, 256, 7, 0, 0, 0, 0, 0, 0, 0);
        d->cmds[n++] = RDP_SYNC_LOAD;
        d->cmds[n++] = RDP_LOAD_TLUT(7, t->tlut_count);
        d->cmds[n++] = RDP_SYNC_PIPE;
    }
    uint32_t lsize = rsize == 3 ? 3 : 2;
    uint32_t texels = w * h;
    uint32_t units = rsize == 0 ? (texels + 3) >> 2 : rsize == 1 ? (texels + 1) >> 1 : texels;
    uint32_t row_bytes = TEX_FORMAT_PIX2BYTES(f, w);
    uint32_t words = row_bytes / 8 ? row_bytes / 8 : 1;
    uint32_t dxt = (2048 + words - 1) / words;
    uint32_t line = rsize == 3 ? (w * 2 + 7) >> 3 : (row_bytes + 7) >> 3;
    d->cmds[n++] = RDP_SET_TEX_IMAGE(rfmt, lsize, 1, PhysicalAddr(t->surf.buffer));
    d->cmds[n++] = RDP_SET_TILE(rfmt, lsize, 0, 0, 7, 0, clamp, 0, maskt, clamp, 0, masks);
    d->cmds[n++] = RDP_SYNC_LOAD;
    d->cmds[n++] = RDP_LOAD_BLOCK(7, units - 1, dxt);
    d->cmds[n++] = RDP_SYNC_PIPE;
    d->cmds[n++] = RDP_SET_TILE(rfmt, rsize, line, 0, 0, 0, clamp, 0, maskt, clamp, 0, masks);
    d->cmds[n++] = RDP_SET_TILE_SIZE(0, w, h);
    d->rdp_bytes = n * 8;
    data_cache_hit_writeback(d, sizeof *d);
    return d;
}

static void upload_tex(uint32_t h, uint32_t list_flags) {
    r3d_tex_t *t = &texs[h];
    int tlut = t->tlut != NULL;
    if (tlut != cur_tlut) {
        /* SOM change: goes through rdpq (one overlay switch). */
        rdpq_mode_tlut(tlut ? TLUT_RGBA16 : TLUT_NONE);
        cur_tlut = tlut;
    }
    uint32_t v = (list_flags & N64_WRAP ? 1 : 0) | (list_flags & N64_BILINEAR ? 2 : 0);
    if (!t->desc[v]) t->desc[v] = build_desc(t, list_flags);
    /* A 0-vertex load = F2_LoadTex (stays in the magma overlay). */
    mg_cmd_write(MG_CMD_LOAD_VERTICES, PhysicalAddr(t->desc[v]), 0);
    cur_tex = h;
    cur_tex_flags = list_flags & (N64_WRAP | N64_BILINEAR);
    stats.tex_loads++;
}

static void draw_inline(const r3d_group_t *g, const n64_r3d_draw_t *d) {
    mg_bind_vertex_buffer(g->verts);
    mg_draw_begin();
    for (uint32_t i = 0; i < g->n_cmds; i++) {
        uint32_t w = g->cmds[i];
        switch (w >> 28) {
        case 1: mg_load_vertices((w >> 12) & 0xFFFF, w & 63, (w >> 6) & 63); break;
        case 2: mg_draw_triangle((w >> 16) & 0xFF, (w >> 8) & 0xFF, w & 0xFF); break;
        /* Bone uniforms by DMA from the frame's arena (written back once per draw). */
        case 3: if (d->bones && (w & 0xFF) < d->n_bones) mg_uniform_load(u_draw, &d->bones[w & 0xFF]); break;
        }
    }
    mg_draw_end();
}

/* The frame uniform currently loaded (the frame's own, or a draw's override: the sky). */
static const n64_r3d_frame_t *cur_frame;
static n64_r3d_frame_t frame_default;

static void load_frame(const n64_r3d_frame_t *f) {
    n64_r3d_frame_t fr = *f;
    fr.clip_text = clip_text;
    fr.clip_size = clip_size;
    fr.modge_text = modge_text;
    fr.modge_size = modge_size;
    mg_uniform_load_inline(u_frame, &fr);
    cur_frame = f;
}

void n64_r3d_begin_impl(const n64_r3d_frame_t *f) {
    frame_default = *f;
    mg_pipeline_bind(pipe);
    mg_set_geometry_flags(MG_GEOMETRY_FLAGS_Z_ENABLED | MG_GEOMETRY_FLAGS_TEX_ENABLED | MG_GEOMETRY_FLAGS_SHADE_ENABLED);
    mg_set_clip_factor(f->guard ? f->guard : 4);
    load_frame(&frame_default);
    if (f->fog_rgba) rdpq_set_fog_color(color_from_packed32(f->fog_rgba));
    cur_tex = 0;
    memset(&stats, 0, sizeof stats);
}

void n64_state_invalidate(void);
void n64_r3d_draw_list_impl(const n64_r3d_list_t *st, const n64_r3d_draw_t *draws, uint32_t n) {
    if (!n) return;
    uint32_t t0 = TICKS_READ();
    apply_list(st);
    uint32_t t1 = TICKS_READ();
    stats.ticks_state += t1 - t0;
    /* CPU batches (main.c) may have loaded TMEM since the last list. */
    cur_tex = 0;
    cur_list_flags = st->flags;
    for (uint32_t i = 0; i < n; i++) {
        const n64_r3d_draw_t *d = &draws[i];
        const n64_r3d_frame_t *want = d->frame ? d->frame : &frame_default;
        if (want != cur_frame) load_frame(want);
        if (!d->bones) {
            /* Loaded by RSP DMA from the caller's per-frame arena (cheaper than inline). */
            data_cache_hit_writeback(d->u, sizeof *d->u);
            mg_uniform_load(u_draw, d->u);
        } else {
            data_cache_hit_writeback(d->bones, d->n_bones * sizeof *d->bones);
        }
        for (uint32_t k = 0; k < d->n_groups; k++) {
            uint32_t gh = d->groups[k], th = d->texs[k];
            if (!gh || gh >= num_groups || !th || th >= num_texs) continue;
            if (st->blend == N64_BLEND_ADD && !(st->flags & N64_TRUE_ADDITIVE)) th = additive_tex(th);
            if (th != cur_tex) upload_tex(th, st->flags);
            const r3d_group_t *g = &groups[gh];
#ifndef F2N64_NO_TRIS /* measurement: everything but the triangles (the ceiling of fewer polygons) */
            if (g->block && !r3d_flat) rspq_block_run(g->block);
            else draw_inline(g, d);
#endif
            stats.groups++;
        }
        stats.draws++;
    }
    stats.ticks_draws += TICKS_READ() - t1;
    n64_state_invalidate();
}

void n64_r3d_stats_impl(n64_r3d_stats_t *out) { *out = stats; }
