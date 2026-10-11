/* Process start-up for the PS5 native app (eboot.bin), linked with the Rust staticlib by
 * tools/ps5/build.sh. Modelled on ps5-native-app-boilerplate's app_crt.cpp (GPL-3.0,
 * BlackBearReloaded / PS5_Vulkan), without its SDK-fork platform layer: the Rust side
 * brings its own heap and libc shims (crates/console-host/src/sce).
 *
 * The console's loader calls _start(process_parameters, loader_teardown); argc is the
 * first int of the parameter block and argv follows at +8.
 * Built with clang --target=x86_64-pc-freebsd12-elf -ffreestanding.
 */

typedef void (*init_fn)(void);
extern init_fn __preinit_array_start[] __attribute__((weak, visibility("hidden")));
extern init_fn __preinit_array_end[] __attribute__((weak, visibility("hidden")));
extern init_fn __init_array_start[] __attribute__((weak, visibility("hidden")));
extern init_fn __init_array_end[] __attribute__((weak, visibility("hidden")));
extern init_fn __fini_array_start[] __attribute__((weak, visibility("hidden")));
extern init_fn __fini_array_end[] __attribute__((weak, visibility("hidden")));
extern void exit(int) __attribute__((noreturn));
extern int atexit(void (*)(void));
extern int main(int, char **, char **);
/* libSceLibcInternal: sets up the system libc (environment, program name) from the
 * process parameters; PS5_Vulkan's app_crt.cpp calls it first, as on hardware. */
extern void _init_env(void *process_parameters);

static void run_forward(init_fn *first, init_fn *last) {
    if (first && last)
        for (; first < last; first++)
            (*first)();
}

void _init(void) {
    run_forward(__preinit_array_start, __preinit_array_end);
    run_forward(__init_array_start, __init_array_end);
}

void _fini(void) {
    if (__fini_array_start && __fini_array_end)
        for (init_fn *f = __fini_array_end; f > __fini_array_start;)
            (*--f)();
}

__attribute__((visibility("default"), noreturn)) void _start(void *process_parameters, void (*loader_teardown)(void)) {
    /* A title starts with flush-to-zero and denormals-are-zero set in MXCSR; the game's
     * float maths (and the deterministic simulation) expect IEEE behaviour as on the PC. */
    unsigned int mxcsr = 0x1f80;
    __asm__ volatile("ldmxcsr %0" : : "m"(mxcsr));
    _init_env(process_parameters);
    int argc = *(int *)process_parameters;
    char **argv = (char **)((char *)process_parameters + 8);
    if (loader_teardown)
        atexit(loader_teardown);
    atexit(_fini);
    _init();
    exit(main(argc, argv, 0));
}

/* memmove, defined here so the binary does not depend on the system libc's (emulators
 * that stand in for the firmware libc may stub it). -ffreestanding keeps the loops from
 * being turned back into memmove calls. */
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
