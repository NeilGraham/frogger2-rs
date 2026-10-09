/* Test-only stub for PC/islutil.h (maths parts copied verbatim). */
#ifndef _ISLUTIL_H_INCLUDE
#define _ISLUTIL_H_INCLUDE
#include <math.h>
unsigned long utilSqrt(unsigned long num);
#define utilCalcAngle(adj,opp) ((int)((atan2(adj,opp) / 6.283185308) * 4096.0))
#endif
