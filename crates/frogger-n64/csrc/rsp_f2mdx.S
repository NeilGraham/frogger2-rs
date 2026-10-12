##############################################################################
# rsp_f2mdx.S - Frogger 2 "MDX" vertex shader for libdragon's magma pipeline.
#
# Reproduces the PC renderer's transform (f2-engine scene.rs = PCPrepareObject +
# PCRenderObject, mdxRender.cpp) on the RSP, two vertices per loop iteration:
#
#   (X, Y, Z, W) = pos * M          M = model x view x P, folded on the CPU (r3d.rs):
#                                   X = vx, Y = vy, W = vz + DIST (DIST = -10),
#                                   Z = a*W + b so that the magma z planes are MDX's
#                                   near/far tests (W > nearClip, W < farClip).
#   q   = floor(W)                  = (long)vz + DIST  (the integer-truncated depth of
#                                     InitOneOverTable / oneOver[])
#   sx  = cx + vx * (-FOV*s / q)    s13.2 quarter pixels: O + S * (X * 0.5/q), S = -8*FOV*s
#   sy  = cy + vy * (-FOV*s / q)
#   z   = Zoff + K * W              linear in vz like the PC's sz = vz * 0.00025
#   rgb = rgba * CMUL + CADD        (alpha_mul, SetObjectGouraudValues)
#   a   = unfogged fraction when fog is on (per vertex: clamp((W - E) * -k) = PC FOG(vz);
#         per object: a constant), the shade alpha of the RDP fog blender (RDPQ_FOG_STANDARD)
#   st  = st * TEX_I + st*TEX_F/65536 + TEX_O   (1/1024-period units -> s10.5 texels)
#
# Face rejection (PCRenderObject) is expressed through magma's per-vertex codes:
#   * trivial-reject byte (RDPQ "REJFLAGS", 1 = inside): bit0 = sx strictly inside
#     (vx0, vx1), bit1 = sy strictly inside (vy0, vy1), bits 2-5 always set. RDPQ rejects a
#     triangle when some bit is clear in all three vertices = MDX's BETWEEN screen reject
#     (no vertex inside on x, or none inside on y).
#   * clip byte: bits 2/5 = outside near/far (MDX stores vz = 0 for such vertices and
#     skips any face using one) -> F2_Clip rejects the triangle; bits 0,1,3,4 = outside the
#     guard band (RDP coordinate range) -> real clipping (rsp_f2clip.S).
#   * back faces: magma culling on the screen-space winding (D3DCULL_CW).
#
# W and 1/W are stored as 1.0: texturing is affine like retail (rhw = 1, no
# TEXTUREPERSPECTIVE), and RDPQ_Triangle then leaves S/T unscaled.
#
# Uniform layout must stay identical in rsp_f2clip.S (shared DMEM) and r3d.c.
##############################################################################
#include <rsp_magma.inc>

#define F2_ATTR_POS     0
#define F2_ATTR_COLOR   1
#define F2_ATTR_ST      2

#include "f2mdx_uniforms.inc"

MgBeginVertexInput
    MgBeginVertexAttribute F2_ATTR_POS, 0
        MgVertexAttributeLoaders LD_POS0, LD_POS1
    MgEndVertexAttribute
    MgBeginVertexAttribute F2_ATTR_COLOR, 0
        MgVertexAttributeLoaders LD_COL0, LD_COL1
    MgEndVertexAttribute
    MgBeginVertexAttribute F2_ATTR_ST, 0
        MgVertexAttributeLoaders LD_ST0, LD_ST1
    MgEndVertexAttribute
MgEndVertexInput

    # Equivalent of MgBeginShader, with our own clipping entry point.
    .section .meta
_MG_META_START:
    .long _mg_uniform_count
    .long _MG_META_UNIFORMS - _MG_META_START
    .long _mg_attribute_count
    .long _MG_META_ATTRIBUTES - _MG_META_START

    .section .text.mg_overlay
    .align 3
_MG_OVERLAY:
_MG_CLIPPING:
    # RDPQ_Triangle_Send_Async jumps here when a vertex has clip bits (t3 = OR of the
    # three vertices' clip halfwords, & 0x3F00). Fixed size: _MG_VTX_SHADER must be at +8.
    j F2_Clip
    nop

_MG_VTX_SHADER:
    # A load of 0 vertices is our texture-load command (a0 = descriptor address).
    andi t0, a1, 0xFFFF
    beqz t0, F2_LoadTex
    nop
    #define vtx_size    t8
    #define vtx_size2   t9
    #define in0         s4
    #define in1         t2
    #define in_end      s6
    #define out0        s7
    #define out1        t3
    #define c0          k0
    #define c1          k1
    #define tr0         a2
    #define tr1         a3

    #define vm0i        $v01
    #define vm0f        $v02
    #define vm1i        $v03
    #define vm1f        $v04
    #define vm2i        $v05
    #define vm2f        $v06
    #define vm3i        $v07
    #define vm3f        $v08
    #define vclipf      $v09
    #define voff        $v10
    #define vscale      $v11
    #define vlo         $v12
    #define vhi         $v13
    #define vcmul       $v14
    #define vcadd       $v15
    #define vtexi       $v16
    #define vtexf       $v17
    #define vfog        $v18
    #define vtexo       $v19
    #define vscr        $v20
    #define vtmp        $v21
    #define vpos        $v22
    #define vcol        $v23
    #define vst         $v24
    #define vcsi        $v25
    #define vcsf        $v26
    #define vinvi       $v27
    #define vinvf       $v28
    #define v___        $v29

    # Command: a0 = id | vertex index (24 bits), a1 = cache slot << 16 | count.
    # Products with MG_VERTEX_SIZE (stride, 40, stride, 0): buffer byte offset, cache byte
    # offset, input byte count (as in libdragon's mgfx).
    addiu t0, rspq_dmem_buf_ptr, %lo(RSPQ_DMEM_BUFFER) - 6
    ldv $v02.e0, 0,t0
    li t1, %lo(MG_VERTEX_SIZE)
    ldv $v01.e0, 0,t1
    vmudh $v04, $v02, $v01
    vsar $v03, COP2_ACC_HI
    vsar $v04, COP2_ACC_MD
    li t1, %lo(RSPQ_SCRATCH_MEM)
    sdv $v04.e0, 0,t1
    ssv $v03.e0, 8,t1
    ssv $v04.e0, 10,t1
    lhu out0, %lo(RSPQ_SCRATCH_MEM + 2)
    lhu t1, %lo(RSPQ_SCRATCH_MEM + 4)
    lw t4, %lo(RSPQ_SCRATCH_MEM + 8)
    lw s0, %lo(MG_VERTEX_BUFFER)
    andi t5, t4, 7
    addu s0, s0, t4
    addu t0, t1, t5
    addiu t0, t0, 7
    andi t0, t0, 0xFF8
    # Input vertices land at the end of the vertex cache (16-byte inputs are always read
    # before the 40-byte outputs overtake them).
    li in0, %lo(MG_VERTEX_CACHE_END)
    subu in0, in0, t0
    jal DMAIn
    addiu t0, t0, -1

    lhu vtx_size, %lo(MG_VERTEX_SIZE)
    sll vtx_size2, vtx_size, 1
    addu in_end, in0, t1
    addu in1, in0, vtx_size
    addiu out0, out0, %lo(MG_VERTEX_CACHE)
    addiu out1, out0, MG_VTX_SIZE

    # M = model x VP (phase 3P: the fold the CPU used to do per draw), s15.16 x s15.16:
    # row r of M = sum_k model[r][k] * VP row k (+ VP row 3 for the translation row), each
    # term a 32x32 product (l, m, n, h partial products), VP rows duplicated in both halves
    # (two vertices per pass) and the model scalars broadcast.
    #define vpi0 $v09
    #define vpf0 $v10
    #define vpi1 $v11
    #define vpf1 $v12
    #define vpi2 $v13
    #define vpf2 $v14
    #define vpi3 $v15
    #define vpf3 $v16
    #define vmri $v17
    #define vmrf $v18
    li t0, %lo(F2_VP_I)
    ldv vpi0.e0, 0x00,t0
    ldv vpi0.e4, 0x00,t0
    ldv vpf0.e0, 0x20,t0
    ldv vpf0.e4, 0x20,t0
    ldv vpi1.e0, 0x08,t0
    ldv vpi1.e4, 0x08,t0
    ldv vpf1.e0, 0x28,t0
    ldv vpf1.e4, 0x28,t0
    ldv vpi2.e0, 0x10,t0
    ldv vpi2.e4, 0x10,t0
    ldv vpf2.e0, 0x30,t0
    ldv vpf2.e4, 0x30,t0
    ldv vpi3.e0, 0x18,t0
    ldv vpi3.e4, 0x18,t0
    ldv vpf3.e0, 0x38,t0
    ldv vpf3.e4, 0x38,t0
    li t0, %lo(F2_MTX_I)
    .macro F2FoldRow row, outi, outf
        ldv vmri.e0, (\row * 8),t0
        ldv vmrf.e0, (0x20 + \row * 8),t0
        vmudl v___, vpf0, vmrf.e0
        vmadm v___, vpi0, vmrf.e0
        vmadn v___, vpf0, vmri.e0
        vmadh v___, vpi0, vmri.e0
        vmadl v___, vpf1, vmrf.e1
        vmadm v___, vpi1, vmrf.e1
        vmadn v___, vpf1, vmri.e1
        vmadh v___, vpi1, vmri.e1
        vmadl v___, vpf2, vmrf.e2
        vmadm v___, vpi2, vmrf.e2
    .if \row == 3
        vmadn v___, vpf2, vmri.e2
        vmadh v___, vpi2, vmri.e2
        vmadn \outf, vpf3, K1
        vmadh \outi, vpi3, K1
    .else
        vmadn \outf, vpf2, vmri.e2
        vmadh \outi, vpi2, vmri.e2
    .endif
    .endm
    F2FoldRow 0, vm0i, vm0f
    F2FoldRow 1, vm1i, vm1f
    F2FoldRow 2, vm2i, vm2f
    F2FoldRow 3, vm3i, vm3f
    #undef vpi0
    #undef vpf0
    #undef vpi1
    #undef vpf1
    #undef vpi2
    #undef vpf2
    #undef vpi3
    #undef vpf3
    #undef vmri
    #undef vmrf

    # Uniforms (each 4-lane vector duplicated in both halves: two vertices per pass).
    ldv vcmul.e0, 0x40,t0
    ldv vcmul.e4, 0x40,t0
    ldv vcadd.e0, 0x48,t0
    ldv vcadd.e4, 0x48,t0
    # Fog: lane 1 = constant (draw), lane 2 = -k, lane 3 = end - 10 (frame).
    ldv vfog.e0, 0x50,t0
    ldv vfog.e4, 0x50,t0
    lhu s1, 0x58(t0)
    lhu s2, 0x5A(t0)
    li s3, 0x10000
    li t0, %lo(MG_CLIP_FACTORS)
    ldv vclipf.e0, 0,t0
    ldv vclipf.e4, 0,t0
    li t0, %lo(F2_TEX_I)
    ldv vtexi.e0, 0,t0
    ldv vtexi.e4, 0,t0
    ldv vtexf.e0, 8,t0
    ldv vtexf.e4, 8,t0
    ldv vtexo.e0, 16,t0
    ldv vtexo.e4, 16,t0
    li t0, %lo(F2_VO)
    ldv voff.e0, 0,t0
    ldv voff.e4, 0,t0
    ldv vscale.e0, 8,t0
    ldv vscale.e4, 8,t0
    ldv vlo.e0, 16,t0
    ldv vlo.e4, 16,t0
    ldv vhi.e0, 24,t0
    ldv vhi.e4, 24,t0
    lsv vfog.e2, 44,t0
    lsv vfog.e3, 46,t0
    lsv vfog.e6, 44,t0
    lsv vfog.e7, 46,t0

F2_VtxLoop:
LD_POS0:
    ldv vpos.e0, 0,in0
LD_POS1:
    ldv vpos.e4, 0,in1
LD_COL0:
    llv vcol.e0, 8,in0
LD_COL1:
    llv vcol.e2, 8,in1
LD_ST0:
    llv vst.e0, 12,in0
LD_ST1:
    llv vst.e4, 12,in1

    # Unpack the 8 colour bytes (A rgba, B rgba) to lanes (value << 7).
    li t0, %lo(RSPQ_SCRATCH_MEM)
    sdv vcol.e0, 0,t0
    luv vcol.e0, 0,t0

    # Clip-space position (s15.16): row-vector pos * M.
    vmudn v___, vm0f, vpos.h0
    vmadh v___, vm0i, vpos.h0
    vmadn v___, vm1f, vpos.h1
    vmadh v___, vm1i, vpos.h1
    vmadn v___, vm2f, vpos.h2
    vmadh v___, vm2i, vpos.h2
    vmadn vcsf, vm3f, K1
    vmadh vcsi, vm3i, K1

    # Guard-band (x, y) and near/far (z) clip codes.
    vmudn vinvf, vcsf, vclipf
    vmadh vinvi, vcsi, vclipf
    vch v___, vinvi, vinvi.h3
    vcl v___, vinvf, vinvf.h3
    cfc2 t4, COP2_CTRL_VCC
    andi t5, t4, 0x707
    srl t6, t5, 5
    or c0, t5, t6
    srl t4, t4, 4
    andi t5, t4, 0x707
    srl t6, t5, 5
    or c1, t5, t6

    # inv = 0.5 / floor(W)  (vrcp of the integer part only: MDX's (long)vz + DIST)
    vrcph v___.e3, vcsi.e3
    vrcpl vinvf.e3, vzero.e3
    vrcph vinvi.e3, vzero.e0
    vrcph v___.e7, vcsi.e7
    vrcpl vinvf.e7, vzero.e7
    vrcph vinvi.e7, vzero.e0

    # t = (X, Y) * inv
    vmudl v___, vcsf, vinvf.h3
    vmadm v___, vcsi, vinvf.h3
    vmadn vtmp, vcsf, vinvi.h3
    vmadh vscr, vcsi, vinvi.h3
    # screen = O + S * t  (s13.2)
    vmudh v___, voff, K1
    vmadn v___, vtmp, vscale
    vmadh vscr, vscr, vscale

    # z = Zoff + K * W (K = Ki + Kf/65536 in lane 2 of vscale / vlo)
    vmudh v___, voff, K1
    vmadl v___, vlo, vcsf.h3
    vmadm v___, vscale, vcsf.h3
    vmadn v___, vlo, vcsi.h3
    vmadh vtmp, vscale, vcsi.h3
    li t0, 0x44
    ctc2 t0, COP2_CTRL_VCC
    vmrg vscr, vtmp, vscr

    # Screen-reject codes: lo < s < hi on x and y (strict, like BETWEEN).
    ctc2 zero, COP2_CTRL_VCO
    vlt v___, vlo, vscr
    cfc2 t4, COP2_CTRL_VCC
    ctc2 zero, COP2_CTRL_VCO
    vlt v___, vscr, vhi
    cfc2 t5, COP2_CTRL_VCC
    and t4, t4, t5
    andi tr0, t4, 3
    ori tr0, tr0, 0x3C
    srl t4, t4, 4
    andi tr1, t4, 3
    ori tr1, tr1, 0x3C

    # Colour: rgba * CMUL + CADD (saturating).
    vmudh v___, vcadd, K1
    vmacf vcol, vcol, vcmul

    # Fog into alpha (masks select: none / constant / per vertex).
    vsub vtmp, vcsi, vfog.h3
    vmudh vtmp, vtmp, vfog.h2
    vge vtmp, vtmp, vzero
    ctc2 s1, COP2_CTRL_VCC
    vmrg vcol, vcol, vfog.h1
    ctc2 s2, COP2_CTRL_VCC
    vmrg vcol, vtmp, vcol

    # Texture coordinates: st * I + st * F / 65536 + O.
    vmudh v___, vtexo, K1
    vmadm v___, vst, vtexf
    vmadh vst, vst, vtexi

    # Store both vertices.
    sdv vscr.e0, MG_VTX_XYZ,out0
    sdv vscr.e4, MG_VTX_XYZ,out1
    sb c0, MG_VTX_CLIP_CODE(out0)
    sb tr0, MG_VTX_TR_CODE(out0)
    sb c1, MG_VTX_CLIP_CODE(out1)
    sb tr1, MG_VTX_TR_CODE(out1)
    suv vcol.e0, MG_VTX_RGBA,out0
    suv vcol.e4, MG_VTX_RGBA,out1
    slv vst.e0, MG_VTX_ST,out0
    slv vst.e4, MG_VTX_ST,out1
    sw s3, MG_VTX_Wi(out0)
    sw s3, MG_VTX_INVWi(out0)
    sw s3, MG_VTX_Wi(out1)
    sw s3, MG_VTX_INVWi(out1)
    sdv vcsi.e0, MG_VTX_CS_POSi,out0
    sdv vcsf.e0, MG_VTX_CS_POSf,out0
    sdv vcsi.e4, MG_VTX_CS_POSi,out1
    sdv vcsf.e4, MG_VTX_CS_POSf,out1

    addu in0, in0, vtx_size2
    addu in1, in1, vtx_size2
    addiu out0, out0, MG_VTX_SIZE2
    sltu t0, in0, in_end
    bnez t0, F2_VtxLoop
    addiu out1, out1, MG_VTX_SIZE2

    j RSPQ_Loop
    nop

##############################################################################
# F2_LoadTex: texture change without leaving the magma overlay.
#
# a0 (low 24 bits) = RDRAM address of a descriptor built by r3d.c (F2_TEXDESC_SIZE bytes):
#   +0  u16 RDP command bytes (n), pad
#   +8  F2_TEX uniform (24 bytes): copied into DMEM
#   +32 n bytes of RDP commands (TLUT + texel loads, tile setup, with syncs), appended to
#       the current RDP dynamic buffer like RDPQ_Send does.
##############################################################################
#define F2_TEXDESC_SIZE 144
F2_LoadTex:
    # A 0-vertex load with a non-zero cache offset is F2_Modge (a1 >> 16 = its size).
    srl t0, a1, 16
    bnez t0, F2_Modge
    nop
    # Wait for the previous triangle's output DMA.
1:  mfc0 t0, COP0_DMA_BUSY
    bnez t0, 1b
    nop
    li s4, %lo(MG_CLIP_CACHE)
    sll s0, a0, 8
    srl s0, s0, 8
    jal DMAIn
    li t0, DMA_SIZE(F2_TEXDESC_SIZE, 1)
    li t2, %lo(MG_CLIP_CACHE)
    ldv $v01.e0, 8,t2
    ldv $v02.e0, 16,t2
    ldv $v03.e0, 24,t2
    li t3, %lo(F2_TEX_I)
    sdv $v01.e0, 0,t3
    sdv $v02.e0, 8,t3
    sdv $v03.e0, 16,t3
    lhu t5, %lo(MG_CLIP_CACHE)
    lw s0, %lo(RDPQ_CURRENT)
    sll s0, s0, 8
    srl s0, s0, 8
    lw a2, %lo(RDPQ_SENTINEL)
    # Everything up to RDPQ_CURRENT is in RDRAM: let the RDP run it.
    lbu t0, %lo(RDPQ_SYNCFULL_ONGOING)
    bnez t0, 2f
    nop
    mtc0 s0, COP0_DP_END
2:  sub t3, a2, t5
    bge t3, s0, F2_LoadTex_Append
    nop
    # Buffer full: switch to the other dynamic buffer (as RDPQ_Send).
    lbu t3, %lo(RDPQ_SYNCFULL_ONGOING)
    ori t3, DP_STATUS_END_VALID
3:  mfc0 t0, COP0_DP_STATUS
    and t0, t0, t3
    bnez t0, 3b
    nop
    lw s0, %lo(RDPQ_DYNAMIC_BUFFERS) + 4
    lw t1, %lo(RDPQ_DYNAMIC_BUFFERS) + 0
    sw s0, %lo(RDPQ_DYNAMIC_BUFFERS) + 0
    sw t1, %lo(RDPQ_DYNAMIC_BUFFERS) + 4
    li s4, %lo(MG_CLIP_CACHE) + 32
    jal DMAOut
    addiu t0, t5, -1
    addu a0, s0, t5
    move a1, s0
    li t0, RDPQ_DYNAMIC_BUFFER_SIZE
    addu a2, s0, t0
    j RSPQCmd_RdpSetBuffer
    li ra, %lo(RSPQ_Loop)
F2_LoadTex_Append:
    li s4, %lo(MG_CLIP_CACHE) + 32
    jal DMAOut
    addiu t0, t5, -1
    addu a0, s0, t5
    j RSPQCmd_RdpAppendBuffer
    li ra, %lo(RSPQ_Loop)

##############################################################################
# F2_Modge: the MODGE/WAVE water animation (r3d.rs `animate_modge`) applied on the RSP.
#
# a0 (low 24 bits) = RDRAM address of a descriptor built by r3d.rs, a1 >> 16 = its size:
#   +0  u32 base vertices (the mesh's stored vertices, 16 bytes each)
#   +4  u32 output (the mesh's vertex buffer, which the vertex loads read)
#   +8  u32 per-vertex u16 position index (readable up to 8 bytes past the end)
#   +12 u16 vertex count, +14 u16 position count, +16 u16 flags (bit 2: write alpha)
#   +24 per position 8 bytes: i16 dy, ds, dt, u8 alpha, pad
# Per vertex: y += dy, s += ds, t += dt (saturating to s16), alpha = its position's
# (with flag 4). Runs between draws, so the vertex cache is scratch here.
##############################################################################
#define MODGE_DESC   MG_VERTEX_CACHE
#define MODGE_PT     (MG_VERTEX_CACHE + 24)
#define MODGE_VBUF   (MG_VERTEX_CACHE + 1056)
#define MODGE_IBUF   (MG_VERTEX_CACHE + 1568)
#define MODGE_CHUNK  32

    # r = clamp(r, -32768, 32767) (r holds a sum of two s16 values).
    .macro F2SatS16 r
    sra t9, \r, 15
    beqz t9, 1f
    addiu t9, t9, 1
    beqz t9, 1f
    nop
    bltz \r, 1f
    li \r, -32768
    li \r, 32767
1:
    .endm

F2_Modge:
    jal DMAWaitIdle
    nop
    sll s0, a0, 8
    srl s0, s0, 8
    srl t0, a1, 16
    addiu t0, t0, -1
    jal DMAIn
    li s4, %lo(MODGE_DESC)
    lw s5, %lo(MODGE_DESC + 0)
    lw s6, %lo(MODGE_DESC + 4)
    lw s7, %lo(MODGE_DESC + 8)
    lhu s3, %lo(MODGE_DESC + 12)
    lhu k0, %lo(MODGE_DESC + 16)
F2_ModgeChunk:
    blez s3, F2_ModgeDone
    move t3, s3
    slti t9, s3, MODGE_CHUNK
    bnez t9, F2_ModgeCount
    nop
    li t3, MODGE_CHUNK
F2_ModgeCount:
    # This chunk's vertices and position indices (rounded up to 8 bytes).
    move s0, s5
    li s4, %lo(MODGE_VBUF)
    sll t0, t3, 4
    jal DMAIn
    addiu t0, t0, -1
    move s0, s7
    li s4, %lo(MODGE_IBUF)
    sll t0, t3, 1
    addiu t0, t0, 7
    andi t0, t0, 0xFF8
    jal DMAIn
    addiu t0, t0, -1
    li t4, %lo(MODGE_VBUF)
    li t5, %lo(MODGE_IBUF)
    sll t6, t3, 4
    addu t6, t6, t4
F2_ModgeVtx:
    lhu t7, 0(t5)
    sll t7, t7, 3
    addiu t7, t7, %lo(MODGE_PT)
    lh t8, 0(t7)
    lh t1, 2(t4)
    addu t1, t1, t8
    F2SatS16 t1
    sh t1, 2(t4)
    lh t8, 2(t7)
    lh t1, 12(t4)
    addu t1, t1, t8
    F2SatS16 t1
    sh t1, 12(t4)
    lh t8, 4(t7)
    lh t1, 14(t4)
    addu t1, t1, t8
    F2SatS16 t1
    sh t1, 14(t4)
    andi t9, k0, 4
    beqz t9, F2_ModgeNoAlpha
    nop
    lbu t8, 6(t7)
    sb t8, 11(t4)
F2_ModgeNoAlpha:
    addiu t4, t4, 16
    bne t4, t6, F2_ModgeVtx
    addiu t5, t5, 2
    move s0, s6
    li s4, %lo(MODGE_VBUF)
    sll t0, t3, 4
    jal DMAOut
    addiu t0, t0, -1
    sll t9, t3, 4
    addu s5, s5, t9
    addu s6, s6, t9
    sll t9, t3, 1
    addu s7, s7, t9
    j F2_ModgeChunk
    subu s3, s3, t3
F2_ModgeDone:
    j RSPQ_Loop
    nop

F2_Clip:
    # A vertex outside near/far: MDX drops the whole face.
    andi t0, t3, 0x2400
    bnez t0, RSPQ_Loop
    lw s0, %lo(F2_CLIP_TEXT)
    # Guard band only: load the clipping overlay (it reloads this shader when done).
    lw t0, %lo(F2_CLIP_SIZE)
    jal_and_j Mg_LoadOverlay, _MG_CLIPPING

    #undef vtx_size
    #undef vtx_size2
    #undef in0
    #undef in1
    #undef in_end
    #undef out0
    #undef out1
    #undef c0
    #undef c1
    #undef tr0
    #undef tr1
