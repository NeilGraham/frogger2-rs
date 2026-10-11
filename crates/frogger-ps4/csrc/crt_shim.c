/* Start-up glue for linking the Rust staticlib against the PS4 system libraries
 * (libkernel + libSceLibcInternal) instead of OpenOrbis' static musl.
 *
 * OpenOrbis' crt1.o calls musl's __libc_start_main; this replaces it: run the
 * .init_array constructors, call main, exit through the system libc.
 * Built by tools/ps4/build-pkg.sh with clang --target=x86_64-pc-freebsd12-elf.
 */

typedef void (*init_fn)(void);
extern init_fn __init_array_start[] __attribute__((weak, visibility("hidden")));
extern init_fn __init_array_end[] __attribute__((weak, visibility("hidden")));
extern void exit(int) __attribute__((noreturn));

int __libc_start_main(int (*main)(int, char **, char **), int argc, char **argv, void (*init)(void), void (*fini)(void), void (*ldso_fini)(void)) {
    (void)init;
    (void)fini;
    (void)ldso_fini;
    if (__init_array_start && __init_array_end) {
        for (init_fn *f = __init_array_start; f < __init_array_end; f++) {
            (*f)();
        }
    }
    exit(main(argc, argv, argv + argc + 1));
}

/* crt1.o references these (weakly); create-fself needs them defined locally. */
int _init(void) { return 0; }
int _fini(void) { return 0; }

/* memmove, defined here so the binary does not depend on the system libc's (emulators
 * without the firmware libc stub it). Compiled with -ffreestanding so the loops are not
 * turned back into memmove calls. */
typedef unsigned long size_t;
void *memmove(void *dst, const void *src, size_t n) {
    unsigned char *d = dst;
    const unsigned char *s = src;
    if (d == s || n == 0)
        return dst;
    if (d < s || d >= s + n) {
        while (n >= 8 && ((size_t)d & 7) == ((size_t)s & 7) && ((size_t)d & 7) != 0) {
            *d++ = *s++;
            n--;
        }
        if (((size_t)d & 7) == 0 && ((size_t)s & 7) == 0) {
            for (; n >= 8; n -= 8, d += 8, s += 8)
                *(unsigned long *)d = *(const unsigned long *)s;
        }
        while (n--)
            *d++ = *s++;
    } else {
        d += n;
        s += n;
        if (((size_t)d & 7) == 0 && ((size_t)s & 7) == 0) {
            for (; n >= 8; n -= 8) {
                d -= 8;
                s -= 8;
                *(unsigned long *)d = *(const unsigned long *)s;
            }
        }
        while (n--)
            *--d = *--s;
    }
    return dst;
}
