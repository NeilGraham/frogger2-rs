##############################################################################
# f2mdx_uniforms.inc - uniform layout shared by rsp_f2mdx.S (vertex shader) and
# rsp_f2clip.S (clipping overlay): both run against the same DMEM uniform region.
# Keep in sync with the C structs in r3d.c.
##############################################################################
MgBeginShaderUniforms
    MgBeginUniform F2_DRAW, 0
        F2_MTX_I:       .ds.b 32    # model rows 0..3 (x, y, z, translation; lanes x, y, z, -), integer halves
        F2_MTX_F:       .ds.b 32    # same, fraction halves (pos_scale folded into rows 0..2)
        F2_CMUL:        .ds.b 8     # r g b a multipliers (s0.15)
        F2_CADD:        .ds.b 8     # r g b a addends (<<7 units, + rounding bias)
        F2_FOGC:        .ds.b 8     # 0, constant unfogged fraction (per-object fog, 0..0x7FFF), 0, 0
        F2_FOGM:        .ds.b 8     # VCC masks: keep-colour (else constant fog), per-vertex fog, 0, 0
    MgEndUniform
    MgBeginUniform F2_TEX, 1
        F2_TEX_I:       .ds.b 8     # s, t integer multipliers
        F2_TEX_F:       .ds.b 8     # s, t fraction multipliers (u0.16)
        F2_TEX_O:       .ds.b 8     # s, t offsets (s10.5)
    MgEndUniform
    MgBeginUniform F2_FRAME, 2
        F2_VO:          .ds.b 8     # x offset (s13.2), y offset, Zoff, 0
        F2_VS:          .ds.b 8     # S, S, K integer, 0
        F2_VLO:         .ds.b 8     # x lo (s13.2), y lo, K fraction, 0
        F2_VHI:         .ds.b 8     # x hi (s13.2), y hi, 0, 0
        F2_WONE:        .ds.b 8     # 1, 0, 1, 0  (W = 1.0, 1/W = 1.0)
        F2_FOGF:        .ds.b 8     # 0, 0, -32767/(end-start), fog end - 10 (in W units)
        F2_VP_I:        .ds.b 32    # view x projection rows 0..3 (lanes X, Y, Z, W), integer halves
        F2_VP_F:        .ds.b 32    # same, fraction halves
        F2_CLIP_TEXT:   .ds.b 4     # RDRAM address of the rsp_f2clip overlay text
        F2_CLIP_SIZE:   .ds.b 4     # its size - 1
        F2_MODGE_TEXT:  .ds.b 4     # RDRAM address of the rsp_f2modge overlay text
        F2_MODGE_SIZE:  .ds.b 4     # its size - 1
    MgEndUniform
    # Pad to the full uniform region so .bss addresses (MG_SCRATCH_MEM, MG_OUTPUT_BUFFER,
    # clip cache) are the same as in the magma core overlay.
    .org _MG_VTX_SHADER_UNIFORMS + MG_MAX_UNIFORMS_SIZE
MgEndShaderUniforms
