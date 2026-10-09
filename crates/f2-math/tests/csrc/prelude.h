/* Forced include for the difftest build of the original C (test-only).
   Emulates MSVC 32-bit `long` on an LP64 host. */
#include <math.h>
#include <stdlib.h>
#include <string.h>
#include <stddef.h>
#include <limits.h>
#include <stdint.h>
#define long int
/* MSVC labs(INT_MIN) == INT_MIN (wrapping); avoid the LP64 libc prototype. */
#define labs(x) (((x) < 0) ? -(x) : (x))
#define PC_VERSION 1
