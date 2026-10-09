/* Test-only support code + wrappers exposing macros to Rust. Not original source. */
#include "maths.h"
#include "newpsx.h"
#include "islutil.h"
#include "pcmisc.h"

fixed gameSpeed = 4096;
IQUATERNION vertQ = {0,4096,0,0};
short bb_acostable[4097];
float rXRes = 640, rYRes = 480;
void XfmPoint(MDX_VECTOR *a, MDX_VECTOR *b, void *c) { (void)c; *a = *b; }
extern ULONG rseed;

/* PC/islutil.cpp utilSqrt: the inline asm re-expressed in C (same bit hack). MSVC6
   _ftol: 64-bit truncation, low 32 bits returned. */
static float n;
unsigned long utilSqrt(unsigned long num)
{
	int bits; float retval;
	n = num;
	memcpy(&bits, &n, 4);
	bits = ((bits - 0x3F800000) >> 1) + 0x3F800000;
	memcpy(&retval, &bits, 4);
	return (unsigned long)(int64_t)(retval*65536);
}

void t_set_rseed(ULONG s) { rseed = s; }
ULONG t_get_rseed(void) { return rseed; }
void t_set_vertq(IQUATERNION *q) { vertQ = *q; }
void t_get_vertq(IQUATERNION *q) { *q = vertQ; }
void t_set_gamespeed(fixed g) { gameSpeed = g; }
fixed t_rsin(int x) { return rsin(x); }
fixed t_rcos(int x) { return rcos(x); }
int t_calc_angle(int a, int b) { return utilCalcAngle(a, b); }
fixed t_fsqrt(long x) { return Fsqrt(x); }
fixed t_fsqrtf(long x) { return FsqrtF(x); }
fixed t_magnitude_v(VECTOR *v) { return MagnitudeV(v); }

#define W3(name, T1, T2, T3) void t_##name(T1 *a, T2 *b, T3 *c) { name(a, b, c); }
#define W2(name, T1, T2) void t_##name(T1 *a, T2 *b) { name(a, b); }
W3(SubVectorVVV, VECTOR, VECTOR, VECTOR)
W3(SubVectorVSV, VECTOR, SVECTOR, VECTOR)
W3(SubVectorVSS, VECTOR, SVECTOR, SVECTOR)
W3(SubVectorSSS, SVECTOR, SVECTOR, SVECTOR)
W3(SubVectorSSF, SVECTOR, SVECTOR, FVECTOR)
W3(SubVectorFSS, FVECTOR, SVECTOR, SVECTOR)
W3(SubVectorFSF, FVECTOR, SVECTOR, FVECTOR)
W3(SubVectorFFS, FVECTOR, FVECTOR, SVECTOR)
W3(SubVectorFFF, FVECTOR, FVECTOR, FVECTOR)
W3(AddVectorSSS, SVECTOR, SVECTOR, SVECTOR)
W3(AddVectorSFS, SVECTOR, FVECTOR, SVECTOR)
W3(AddVectorSSF, SVECTOR, SVECTOR, FVECTOR)
W3(AddVectorSFF, SVECTOR, FVECTOR, FVECTOR)
W3(AddVectorFFS, FVECTOR, FVECTOR, SVECTOR)
W3(AddVectorFFF, FVECTOR, FVECTOR, FVECTOR)
W3(AddVectorVSF, VECTOR, SVECTOR, FVECTOR)
W3(CrossProductFFF, FVECTOR, FVECTOR, FVECTOR)
W2(SubFromVectorVS, VECTOR, SVECTOR)
W2(SubFromVectorSS, SVECTOR, SVECTOR)
W2(SubFromVectorSF, SVECTOR, FVECTOR)
W2(SubFromVectorFS, FVECTOR, SVECTOR)
W2(SubFromVectorFF, FVECTOR, FVECTOR)
W2(SetVectorVS, VECTOR, SVECTOR)
W2(SetVectorSV, SVECTOR, VECTOR)
W2(SetVectorSF, SVECTOR, FVECTOR)
W2(SetVectorFS, FVECTOR, SVECTOR)
W2(SetVectorFV, FVECTOR, VECTOR)
W2(AddToVectorVS, VECTOR, SVECTOR)
W2(AddToVectorSS, SVECTOR, SVECTOR)
W2(AddToVectorFS, FVECTOR, SVECTOR)
W2(AddToVectorFV, FVECTOR, VECTOR)
W2(AddToVectorSF, SVECTOR, FVECTOR)
fixed t_DotProductFF(FVECTOR *a, FVECTOR *b) { return DotProductFF(a, b); }
int t_DotProductSS(SVECTOR *a, SVECTOR *b) { return DotProductSS(a, b); }
void t_ScaleVectorFF(FVECTOR *v, fixed s) { ScaleVectorFF(v, s); }
void t_ScaleVectorSF(SVECTOR *v, fixed s) { ScaleVectorSF(v, s); }
void t_ScaleVectorS(SVECTOR *v, int s) { ScaleVector(v, s); }
void t_ScaleVectorF(FVECTOR *v, int s) { ScaleVector(v, s); }
void t_LinearInterpFSSF(FVECTOR *r, SVECTOR *a, SVECTOR *b, fixed t) { LinearInterpFSSF(r, a, b, t); }
void t_AddScaledVector2D(VECTOR *r, VECTOR *a, int s1, VECTOR *b, int s2) { AddScaledVector2D(r, a, s1, b, s2); }
void t_AddOneScaledVector(FVECTOR *r, FVECTOR *a, FVECTOR *b, int s) { AddOneScaledVector(r, a, b, s); }
void t_AddToOneScaledVector(FVECTOR *r, FVECTOR *b, fixed s) { AddToOneScaledVector(r, b, s); }
void t_AddToOneScaledVector2D(FVECTOR *r, FVECTOR *b, int s) { AddToOneScaledVector2D(r, b, s); }
void t_CrossProductVertical(FVECTOR *r, FVECTOR *b) { CrossProductVertical(r, b); }
int t_DotProduct2D(FVECTOR *a, FVECTOR *b) { return DotProduct2D(a, b); }
