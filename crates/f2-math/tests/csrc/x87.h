/* Test-only (not original source): the retail MSVC6 x87 instruction sequences of the PC
   float macros, executed on the host's x87 FPU under Direct3D's control word 0x007F
   (24-bit precision, round to nearest). `build.rs` routes `rsin`/`rcos` (PC/libgte.h),
   `utilCalcAngle` (PC/islutil.h) and `arccos` (Common/maths.c) here so the difftests check
   the Rust PC24 emulation against real hardware. x86-64 gcc/clang only. */
#ifndef F2_X87_H
#define F2_X87_H

static const unsigned short x87_cw_near = 0x007F, x87_cw_chop = 0x0C7F;
static const float x87_inv4096f = 1.0f / 4096.0f;
static const double x87_pi2 = 3.141592654 * 2.0;
static const double x87_d4096 = 4096.0;
static const double x87_twopi_c = 6.283185308;
static const float x87_f4096 = 4096.0f;
static const double x87_k = 4096 / (3.141592654 * 2.0);

/* `_ftol` = fistp qword with round-to-zero; `(long)` keeps the low 32 bits. */
#define X87_SEQ(body, ...)                                                                    \
	int64_t r; unsigned short save; /* not `long long`: prelude.h #defines long */                                                     \
	__asm__ volatile("fnstcw %[save]\n fldcw %[cn]\n" body "\n fldcw %[cc]\n fistpll %[r]\n fldcw %[save]\n" \
		: [r] "=m"(r), [save] "=m"(save)                                                      \
		: [cn] "m"(x87_cw_near), [cc] "m"(x87_cw_chop), __VA_ARGS__                           \
		: "st", "st(1)", "st(2)");                                                            \
	return (int)r;

static inline int x87_rsin(int x) {
	X87_SEQ("fildl %[x]\n fmuls %[inv]\n fmull %[pi2]\n fsin\n fmull %[k]",
		[x] "m"(x), [inv] "m"(x87_inv4096f), [pi2] "m"(x87_pi2), [k] "m"(x87_d4096))
}
static inline int x87_rcos(int x) {
	X87_SEQ("fildl %[x]\n fmuls %[inv]\n fmull %[pi2]\n fcos\n fmull %[k]",
		[x] "m"(x), [inv] "m"(x87_inv4096f), [pi2] "m"(x87_pi2), [k] "m"(x87_d4096))
}
static inline int x87_calc_angle(int adj, int opp) {
	X87_SEQ("fildl %[a]\n fildl %[o]\n fpatan\n fdivl %[c]\n fmull %[k]",
		[a] "m"(adj), [o] "m"(opp), [c] "m"(x87_twopi_c), [k] "m"(x87_d4096))
}
/* MSVC6 CRT acos: atan2(sqrt(1-x*x), x) via fpatan. */
static inline int x87_arccos(int c) {
	X87_SEQ("fildl %[c]\n fdivs %[f]\n fld %%st(0)\n fmul %%st(0), %%st(0)\n fld1\n fsubp\n fsqrt\n fxch\n fpatan\n fmull %[k]",
		[c] "m"(c), [f] "m"(x87_f4096), [k] "m"(x87_k))
}
#endif
