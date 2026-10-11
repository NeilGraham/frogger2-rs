// Host benchmark of libdragon's asset decompressors (LZ4 / aPLib / Shrinkler, the C
// reference implementations from the libdragon tree) on mkasset output files.
// Built by `f2-n64pack --bench` with: gcc -O2 -I$LIBDRAGON/src -I$LIBDRAGON/include
// -I$LIBDRAGON/tools/common ldcomp.c
// usage: ldcomp <reps> <file.asset>...   prints "<files> <raw bytes> <seconds per pass>"
#define _GNU_SOURCE
#include <stdio.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#undef MIN
#undef MAX
#include "../../src/utils.c"
#include "../../src/compress/aplib_dec.c"
#include "../../src/compress/shrinkler_dec.c"
#include "../../src/compress/lz4_dec.c"
#include "../../src/compress/ringbuf.c"

static uint8_t *slurp(const char *fn, long *size) {
    FILE *f = fopen(fn, "rb");
    if (!f) { perror(fn); exit(1); }
    fseek(f, 0, SEEK_END);
    *size = ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *b = malloc(*size + 16);
    if (fread(b, 1, *size, f) != (size_t)*size) { perror(fn); exit(1); }
    fclose(f);
    return b;
}

static uint64_t varint(const uint8_t **p) {
    uint64_t v = 0; int sh = 0;
    for (;;) { uint8_t b = *(*p)++; v |= (uint64_t)(b & 0x7f) << sh; if (!(b & 0x80)) break; sh += 7; }
    return v;
}

int main(int argc, char **argv) {
    int reps = atoi(argv[1]);
    double secs = 0; uint64_t raw = 0; int nfiles = 0;
    for (int i = 2; i < argc; i++) {
        long sz; uint8_t *f = slurp(argv[i], &sz);
        if (sz < 5 || memcmp(f, "DCA", 3) != 0) { // stored uncompressed by mkasset
            raw += sz; nfiles++; free(f); continue;
        }
        int algo = (f[4] >> 4) & 3;
        const uint8_t *p = f + 5;
        uint32_t cmp = varint(&p), orig = varint(&p), margin = varint(&p); (void)margin;
        long hs = p - f; if (hs & 1) hs++;
        uint8_t *src = malloc(cmp + 16); memcpy(src, f + hs, cmp);
        uint8_t *dst = malloc(orig + 64);
        struct timespec t0, t1;
        clock_gettime(CLOCK_MONOTONIC, &t0);
        for (int r = 0; r < reps; r++) {
            int n = -1;
            if (algo == 1) n = decompress_lz4_full_inplace(src, cmp, dst, orig);
            else if (algo == 2) n = decompress_aplib_full_inplace(src, cmp, dst, orig);
            else if (algo == 3) n = decompress_shrinkler_full_inplace(src, cmp, dst, orig);
            if (n != (int)orig) { fprintf(stderr, "%s: decoded %d of %u\n", argv[i], n, orig); exit(1); }
        }
        clock_gettime(CLOCK_MONOTONIC, &t1);
        secs += ((t1.tv_sec - t0.tv_sec) + (t1.tv_nsec - t0.tv_nsec) * 1e-9) / reps;
        raw += orig; nfiles++;
        free(src); free(dst); free(f);
    }
    printf("%d %llu %.9f\n", nfiles, (unsigned long long)raw, secs);
    return 0;
}
