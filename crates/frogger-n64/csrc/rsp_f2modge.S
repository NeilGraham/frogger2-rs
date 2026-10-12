############################################################
# rsp_f2modge.S - MODGE/WAVE water overlay of the Frogger 2 MDX shader (rsp_f2mdx.S).
#
# The shader's F2_ModgeLoad DMAs this overlay's text over its own (from _MG_OVERLAY, like
# the clipping overlay rsp_f2clip.S) and enters F2_Modge at _MG_OVERLAY + 16; the next vertex
# load reloads the shader through the bootstrapper at _MG_VTX_SHADER, a clipping request
# through Mg_Clipping below. Kept out of the shader for IMEM space.
############################################################
#include <rsp_magma.inc>

#include "f2mdx_uniforms.inc"

_MgBeginShader
    # The next vertex load reloads the shader.
    _MgBootstrapShader
    # Entry (_MG_OVERLAY + 16; r3d.c checks the offset).
F2_ModgeEntry:
    j F2_Modge
    nop

Mg_Clipping:
    # A triangle needing clipping (none follows this command, but to be safe): back to the
    # shader and its clipping entry.
    jal_and_j Mg_LoadShader, _MG_CLIPPING

##############################################################################
# F2_Modge: the MODGE/WAVE water animation (r3d.rs `animate_modge`) on the RSP: per position
# the offsets from the x and z phase summands the CPU computed (phase A), then per vertex
# (phase B). Bit-exact model: r3d.rs `modge_pair16` / `modge_apply`.
#
# a0 (low 24 bits) = RDRAM address of a descriptor built by r3d.rs, a1 >> 16 = its size:
#   +0  u32 base vertices (the mesh's stored vertices, 16 bytes each)
#   +4  u32 output (the mesh's vertex buffer, which the vertex loads read)
#   +8  u32 per-vertex u16 position index (readable up to 8 bytes past the end)
#   +12 u16 vertex count, +14 u16 position count, +16 u16 flags (bit 2: write alpha)
#   +18 s16 y multiplier, +20 s16 s/t multiplier (VMULF, Q15), +22 u16 DMEM offset of the
#       position table (8 bytes per position, room for a multiple of 16)
#   +24 u32 position list: per position u16 offsets (in this descriptor) of its x and z
#       terms, u16 stored alpha << 7, pad
#   +28 u32 sine table (Q14, 257 entries over one turn, padded to 520 bytes)
#   +32 x terms, then z terms: 3 x u16 phase summands (2^16 = one turn) + pad each
# Phase A per position: S = (sin + 1) Q10 of each summed phase (table, 8-bit interpolation),
#   dy = S_y*ky, ds = S_x*kst, dt = S_z*kst (VMULF), alpha = a0*(1280 + S_x + S_z)/3328.
# Phase B per vertex: y += dy, s += ds, t += dt (saturating to s16), alpha = its position's
#   (with flag 4).
# Runs between draws: the vertex cache (and the clip cache, for staging) are scratch.
##############################################################################
#define MODGE_DESC   MG_VERTEX_CACHE
#define MODGE_TAB_BYTES 520
#define MODGE_VBUF   (MG_VERTEX_CACHE + 0)
#define MODGE_IBUF   (MG_VERTEX_CACHE + 512)
#define MODGE_CHUNK  32
    # Staging for 8 positions (16-byte aligned): per sine j at 48*j: table value, slope,
    # fraction << 8; stored alpha << 7 at 144; constants at 160; results at 176..239.
#define MODGE_ST     MG_CLIP_CACHE
#define vK           $v20

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

    # (sin + 1) Q10 of staged sine j into vector register vd.
    .macro F2ModgeSin j, vd
    lqv $v01, (48*\j),t8
    lqv $v02, (48*\j + 16),t8
    lqv $v03, (48*\j + 32),t8
    vmudm $v02, $v02, $v03
    vadd $v01, $v01, $v02
    vadd $v01, $v01, vK.e0
    vmudm $v01, $v01, vK.e1
    vadd \vd, $v01, vK.e2
    .endm

F2_Modge:
    jal DMAWaitIdle
    nop
    # Descriptor (header + terms).
    sll s0, a0, 8
    srl s0, s0, 8
    srl v1, a1, 16
    addiu t0, v1, -1
    jal DMAIn
    li s4, %lo(MODGE_DESC)
    lw s1, %lo(MODGE_DESC + 0)
    lw s2, %lo(MODGE_DESC + 4)
    lw s3, %lo(MODGE_DESC + 8)
    lhu s5, %lo(MODGE_DESC + 12)
    lhu a3, %lo(MODGE_DESC + 14)
    lhu k0, %lo(MODGE_DESC + 16)
    lhu s6, %lo(MODGE_DESC + 22)
    addiu s6, s6, %lo(MODGE_DESC)
    lw a2, %lo(MODGE_DESC + 24)
    # Constants: 8, 4096, 1024, ky, kst, 5041, 1280, 255.
    li t8, %lo(MODGE_ST)
    li t4, 8
    sh t4, 160(t8)
    li t4, 4096
    sh t4, 162(t8)
    li t4, 1024
    sh t4, 164(t8)
    lhu t4, %lo(MODGE_DESC + 18)
    sh t4, 166(t8)
    lhu t4, %lo(MODGE_DESC + 20)
    sh t4, 168(t8)
    li t4, 5041
    sh t4, 170(t8)
    li t4, 1280
    sh t4, 172(t8)
    li t4, 255
    sh t4, 174(t8)
    lqv vK, 160,t8
    # Sine table after the descriptor, then the position input chunk.
    lw s0, %lo(MODGE_DESC + 28)
    addiu s7, v1, %lo(MODGE_DESC)
    move s4, s7
    jal DMAIn
    li t0, MODGE_TAB_BYTES - 1
    addiu v1, s7, MODGE_TAB_BYTES
    move v0, s6
F2_ModgePosChunk:
    blez a3, F2_ModgeVerts
    move t9, a3
    slti t4, a3, 16
    bnez t4, F2_ModgePosCount
    nop
    li t9, 16
F2_ModgePosCount:
    move s0, a2
    move s4, v1
    sll t0, t9, 3
    jal DMAIn
    addiu t0, t0, -1
    sll t4, t9, 3
    addu a2, a2, t4
    subu a3, a3, t9
    # Two passes of 8 (the table has room for a multiple of 16 positions).
    jal F2_ModgeEight
    move t7, v1
    jal F2_ModgeEight
    nop
    j F2_ModgePosChunk
    nop

F2_ModgeEight:
    # Gather: per lane the table value, slope and fraction of its three summed phases.
    li t8, %lo(MODGE_ST)
F2_ModgeLane:
    lhu t0, 0(t7)
    lhu t1, 2(t7)
    lhu t2, 4(t7)
    sh t2, 144(t8)
    addiu t0, t0, %lo(MODGE_DESC)
    addiu t1, t1, %lo(MODGE_DESC)
    # Per summed phase (3): table value, slope, fraction << 8.
    move t6, t8
    li k1, 3
1:  lhu t3, 0(t0)
    lhu t4, 0(t1)
    addu t3, t3, t4
    srl t4, t3, 8
    andi t4, t4, 0xFF
    sll t4, t4, 1
    addu t4, t4, s7
    lh t5, 0(t4)
    lh t4, 2(t4)
    subu t4, t4, t5
    sh t5, 0(t6)
    sh t4, 16(t6)
    andi t3, t3, 0xFF
    sll t3, t3, 8
    sh t3, 32(t6)
    addiu t0, t0, 2
    addiu t1, t1, 2
    addiu k1, k1, -1
    bnez k1, 1b
    addiu t6, t6, 48
    addiu t8, t8, 2
    li t3, %lo(MODGE_ST) + 16
    bne t8, t3, F2_ModgeLane
    addiu t7, t7, 8
    # Vector part: 8 positions.
    li t8, %lo(MODGE_ST)
    ctc2 zero, COP2_CTRL_VCO
    F2ModgeSin 0, $v04
    F2ModgeSin 1, $v05
    F2ModgeSin 2, $v06
    vmulf $v07, $v04, vK.e4
    vmulf $v08, $v05, vK.e4
    vmulf $v09, $v06, vK.e3
    vadd $v10, $v04, $v05
    vadd $v10, $v10, vK.e6
    lqv $v11, 144,t8
    vmudm $v11, $v11, $v10
    vmulf $v11, $v11, vK.e5
    vlt $v11, $v11, vK.e7
    # Out: per position dy, ds, dt, alpha (transposed through the staging area).
    sqv $v09, 176,t8
    sqv $v07, 192,t8
    sqv $v08, 208,t8
    sqv $v11, 224,t8
    li t3, 8
1:  lh t4, 176(t8)
    sh t4, 0(v0)
    lh t4, 192(t8)
    sh t4, 2(v0)
    lh t4, 208(t8)
    sh t4, 4(v0)
    lh t4, 224(t8)
    sh t4, 6(v0)
    addiu t8, t8, 2
    addiu t3, t3, -1
    bnez t3, 1b
    addiu v0, v0, 8
    jr ra
    nop

F2_ModgeVerts:
    # Phase B (the vertex buffers overlay the descriptor area; the table is at s6).
    blez s5, F2_ModgeDone
    move t3, s5
    slti t9, s5, MODGE_CHUNK
    bnez t9, F2_ModgeCount
    nop
    li t3, MODGE_CHUNK
F2_ModgeCount:
    # This chunk's vertices and position indices (rounded up to 8 bytes).
    move s0, s1
    li s4, %lo(MODGE_VBUF)
    sll t0, t3, 4
    jal DMAIn
    addiu t0, t0, -1
    move s0, s3
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
    addu t7, t7, s6
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
    lbu t8, 7(t7)
    sb t8, 11(t4)
F2_ModgeNoAlpha:
    addiu t4, t4, 16
    bne t4, t6, F2_ModgeVtx
    addiu t5, t5, 2
    move s0, s2
    li s4, %lo(MODGE_VBUF)
    sll t0, t3, 4
    jal DMAOut
    addiu t0, t0, -1
    sll t9, t3, 4
    addu s1, s1, t9
    addu s2, s2, t9
    sll t9, t3, 1
    addu s3, s3, t9
    j F2_ModgeVerts
    subu s5, s5, t3
F2_ModgeDone:
    j RSPQ_Loop
    nop

