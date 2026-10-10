/* Test-only stub for PC/islutil.h (maths parts copied verbatim). */
#ifndef _ISLUTIL_H_INCLUDE
#define _ISLUTIL_H_INCLUDE
#include <math.h>
unsigned long utilSqrt(unsigned long num);
/* Original: ((int)((atan2(adj,opp) / 6.283185308) * 4096.0)), run as its retail x87 PC24
   sequence (x87.h). */
#define utilCalcAngle(adj,opp) x87_calc_angle(adj, opp)
#endif
