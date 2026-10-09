/* Test-only stub for PC/pcmisc.h: just enough for TransformPoint to compile. */
typedef struct { float vx, vy, vz; } MDX_VECTOR;
#define SetVectorRS(a,b) ((a)->vx = (b)->vx, (a)->vy = (b)->vy, (a)->vz = (b)->vz)
void XfmPoint(MDX_VECTOR *a, MDX_VECTOR *b, void *c);
extern float rXRes, rYRes;
