# o64 (libdragon, GCC) <-> n32 (Rust, LLVM) call bridges. See n64_ffi.h and
# tools/n64/README.md "ABI".
#
# Differences bridged here (everything else - $a0-$a3 for the first four integer/pointer
# arguments, $v0 result, 64-bit callee-saved $s0-$s8/$fp, 32-bit pointers, FR=1 FPU - is
# common to both ABIs):
#   * o64 callees may store their register arguments into a 32-byte home area at the top
#     of the caller's frame (0..31($sp) at entry); n32 callers do not reserve it.
#   * o64 preserves all of $f20-$f31; n32 only the even ones.
#   * o64 keeps $sp 8-byte aligned; n32 assumes 16.
#   * libdragon runs with the overflow / divide-by-zero / invalid FPU traps enabled.

    .set noreorder
    .set noat
    .text

# Rust -> C stub: `name` (called by Rust) calls `name_impl` (C) with a home area.
.macro RUST_TO_C name
    .globl \name
    .type \name, @function
    .align 5
\name:
    addiu $sp, $sp, -48
    sd $ra, 40($sp)
    jal \name\()_impl
    nop
    ld $ra, 40($sp)
    jr $ra
    addiu $sp, $sp, 48
    .size \name, . - \name
.endm

    RUST_TO_C n64_log
    RUST_TO_C n64_panic
    RUST_TO_C n64_alloc
    RUST_TO_C n64_free
    RUST_TO_C n64_ticks
    RUST_TO_C n64_pad
    RUST_TO_C n64_file_read
    RUST_TO_C n64_tex_create
    RUST_TO_C n64_begin_frame
    RUST_TO_C n64_draw_tris
    RUST_TO_C n64_draw_tl_tris
    RUST_TO_C n64_present
    RUST_TO_C n64_rdp_wait
    RUST_TO_C n64_set_fps_limit
    RUST_TO_C n64_beep
    RUST_TO_C n64_audio_stats
    RUST_TO_C n64_memory_size
    RUST_TO_C n64_probe_c
    RUST_TO_C n64_tex_create2
    RUST_TO_C n64_tex_free
    RUST_TO_C n64_draw_rects
    RUST_TO_C n64_cache_writeback
    RUST_TO_C n64_heap_stats
    RUST_TO_C n64_sfx_open
    RUST_TO_C n64_sfx_close
    RUST_TO_C n64_sfx_channels
    RUST_TO_C n64_sfx_play
    RUST_TO_C n64_ch_set
    RUST_TO_C n64_ch_stop
    RUST_TO_C n64_ch_playing
    RUST_TO_C n64_music_play
    RUST_TO_C n64_music_vol
    RUST_TO_C n64_audio_poll
    RUST_TO_C n64_prof
    RUST_TO_C n64_r3d_init
    RUST_TO_C n64_r3d_tex_create
    RUST_TO_C n64_r3d_group_create
    RUST_TO_C n64_r3d_begin
    RUST_TO_C n64_r3d_draw_list
    RUST_TO_C n64_r3d_stats
    RUST_TO_C n64_tex_update
    RUST_TO_C n64_tex_update_begin
    RUST_TO_C n64_tex_update_end
    RUST_TO_C n64_r3d_tex_alias
    RUST_TO_C n64_r3d_reset
    RUST_TO_C n64_r3d_counts
    RUST_TO_C n64_tex_create_ntx
    RUST_TO_C n64_sfx_freq
    RUST_TO_C n64_fmv_play
    RUST_TO_C n64_rcp_load
    RUST_TO_C n64_save_read
    RUST_TO_C n64_audio_gap_take
    RUST_TO_C n64_audio_poll_due
    RUST_TO_C n64_save_write

# void f2n64_call(void (*fn)(void)) - C -> Rust.
    .globl f2n64_call
    .type f2n64_call, @function
    .align 5
f2n64_call:
    addiu $sp, $sp, -128
    sd $ra, 0($sp)
    sd $s0, 8($sp)
    sdc1 $f20, 16($sp)
    sdc1 $f21, 24($sp)
    sdc1 $f22, 32($sp)
    sdc1 $f23, 40($sp)
    sdc1 $f24, 48($sp)
    sdc1 $f25, 56($sp)
    sdc1 $f26, 64($sp)
    sdc1 $f27, 72($sp)
    sdc1 $f28, 80($sp)
    sdc1 $f29, 88($sp)
    sdc1 $f30, 96($sp)
    sdc1 $f31, 104($sp)
    cfc1 $t0, $31
    sw $t0, 112($sp)
    li $t1, ~0xF80           # FCR31 enable bits (I U O Z V) off
    and $t1, $t0, $t1
    ctc1 $t1, $31
    move $s0, $sp
    li $t2, -16
    and $sp, $sp, $t2
    jalr $a0
    nop
    move $sp, $s0
    lw $t0, 112($sp)
    li $t1, ~0x3F000         # drop cause bits raised by Rust so re-enabling cannot trap
    and $t0, $t0, $t1
    ctc1 $t0, $31
    ldc1 $f20, 16($sp)
    ldc1 $f21, 24($sp)
    ldc1 $f22, 32($sp)
    ldc1 $f23, 40($sp)
    ldc1 $f24, 48($sp)
    ldc1 $f25, 56($sp)
    ldc1 $f26, 64($sp)
    ldc1 $f27, 72($sp)
    ldc1 $f28, 80($sp)
    ldc1 $f29, 88($sp)
    ldc1 $f30, 96($sp)
    ldc1 $f31, 104($sp)
    ld $s0, 8($sp)
    ld $ra, 0($sp)
    jr $ra
    addiu $sp, $sp, 128
    .size f2n64_call, . - f2n64_call
