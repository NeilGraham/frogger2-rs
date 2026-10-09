/* Test-only stub replacing Common/types.h: only the maths-related declarations. */
#ifndef TYPES_H_INCLUDED
#define TYPES_H_INCLUDED
#include "fixed.h"
#define PI2 (3.141592654*2.0F)   /* MDX Library/mdxMath.h:12 */
#include "libgte.h"              /* PC/libgte.h (original) */
#define X 0
#define Y 1
#define Z 2
#define BYTE char
#define SHORT short
#define LONG long
#define UBYTE unsigned char
#define ULONG unsigned long
#define USHORT unsigned short
typedef int BOOL;
#define TRUE 1
#define FALSE 0
#define Max(p1,p2) (((p1) > (p2)) ? (p1) : (p2))
#define Min(p1,p2) (((p1) > (p2)) ? (p2) : (p1))
typedef struct { LONG x, y, z, w; } IQUATERNION;
typedef struct { LONG matrix[4][4]; } MATRIXI;
typedef struct { fixed x,z; } POINT2D;
typedef struct { SVECTOR pos; VECTOR size; } BOX;
VECTOR MakeVECTOR(long x, long y, long z);
SVECTOR MakeSVECTOR(short x, short y, short z);
FVECTOR MakeFVECTOR(fixed x, fixed y, fixed z);
#endif
