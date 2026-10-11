/* Emulated thread-local storage for the PS4 / PS5 builds.
 *
 * The Rust code (std included) is compiled with `tls-model: emulated`
 * (tools/ps4/console-target.py): each thread local is a `__emutls_v.<name>` control block
 * and every access calls `__emutls_get_address`, which keeps a per-thread array of the
 * thread's copies under one pthread key. The executable then has no TLS segment, so
 * nothing depends on how the console's loader lays out the main executable's static TLS
 * block (homebrew titles that run on PS5 hardware use emulated TLS for the same reason:
 * the PS5 Clang target defaults to it, PS5_Vulkan / UnleashedRecomp-Prospero build with
 * -femulated-tls).
 *
 * The algorithm is compiler-rt's lib/builtins/emutls.c (Apache-2.0 WITH LLVM-exception),
 * with the destructor ordering fix of UnleashedRecomp-Prospero's ps5-emutls-cxa.c
 * (tools/radv-link.sh there, GPL-3.0-or-later): the console's libkernel runs pthread key
 * destructors in key order, and the emutls key is created first (on the first thread-local
 * access), before std's key that runs Rust's thread-local destructors
 * (std/src/sys/thread_local/guard/key.rs). Freeing the thread's copies in the first round
 * would let those destructors touch freed memory, so the emutls array survives
 * EMUTLS_SKIP_DESTRUCTOR_ROUNDS rounds (FreeBSD runs up to PTHREAD_DESTRUCTOR_ITERATIONS
 * = 4) before it is released.
 *
 * Freestanding (no system headers): only pthread keys, the heap and abort are used. The
 * one-time key creation and object index assignment use atomics, not pthread_once /
 * mutexes, whose struct layouts differ between the SDK headers.
 * Built by tools/ps4/build-pkg.sh and tools/ps5/build.sh with
 * clang --target=x86_64-pc-freebsd12-elf -ffreestanding -fPIC -O2.
 */

typedef unsigned long uintptr_t;
typedef unsigned long size_t;
typedef int pthread_key_t;

extern int pthread_key_create(pthread_key_t *key, void (*destructor)(void *));
extern void *pthread_getspecific(pthread_key_t key);
extern int pthread_setspecific(pthread_key_t key, const void *value);
extern int sched_yield(void);
extern void *malloc(size_t n);
extern void *realloc(void *p, size_t n);
extern void free(void *p);
extern void abort(void) __attribute__((noreturn));

#define EMUTLS_SKIP_DESTRUCTOR_ROUNDS 2

typedef struct emutls_address_array {
    uintptr_t skip_destructor_rounds;
    uintptr_t size; /* number of elements in data */
    void *data[];
} emutls_address_array;

/* The control block clang/LLVM emits for each thread local (`__emutls_v.<name>`). */
typedef struct __emutls_control {
    size_t size;  /* object size */
    size_t align; /* object alignment */
    union {
        uintptr_t index; /* 1-based index into the per-thread array, 0 = not assigned */
        void *address;
    } object;
    void *value; /* initial value, or 0 for zero-initialised */
} __emutls_control;

static pthread_key_t emutls_key;
static int emutls_key_state; /* 0 = none, 1 = being created, 2 = ready */
static int emutls_index_lock;
static uintptr_t emutls_num_object;

static void emutls_fill(void *dst, const void *src, size_t n) {
    unsigned char *d = dst;
    const unsigned char *s = src;
    if (s)
        while (n--)
            *d++ = *s++;
    else
        while (n--)
            *d++ = 0;
}

static void *emutls_memalign_alloc(size_t align, size_t size) {
    if (align < sizeof(void *))
        align = sizeof(void *);
    size_t extra = align - 1 + sizeof(void *);
    char *object = (char *)malloc(extra + size);
    if (!object)
        abort();
    void *base = (void *)(((uintptr_t)(object + extra)) & ~(uintptr_t)(align - 1));
    ((void **)base)[-1] = object;
    return base;
}

static void emutls_memalign_free(void *base) {
    free(((void **)base)[-1]);
}

static void emutls_key_destructor(void *ptr) {
    emutls_address_array *array = (emutls_address_array *)ptr;
    if (array->skip_destructor_rounds > 0) {
        /* Keep the thread's copies for the later rounds, in which std's destructors run. */
        array->skip_destructor_rounds--;
        pthread_setspecific(emutls_key, array);
        return;
    }
    pthread_setspecific(emutls_key, 0);
    for (uintptr_t i = 0; i < array->size; ++i)
        if (array->data[i])
            emutls_memalign_free(array->data[i]);
    free(array);
}

static void emutls_init_once(void) {
    if (__atomic_load_n(&emutls_key_state, __ATOMIC_ACQUIRE) == 2)
        return;
    int expected = 0;
    if (__atomic_compare_exchange_n(&emutls_key_state, &expected, 1, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
        if (pthread_key_create(&emutls_key, emutls_key_destructor) != 0)
            abort();
        __atomic_store_n(&emutls_key_state, 2, __ATOMIC_RELEASE);
        return;
    }
    while (__atomic_load_n(&emutls_key_state, __ATOMIC_ACQUIRE) != 2)
        sched_yield();
}

static uintptr_t emutls_get_index(__emutls_control *control) {
    uintptr_t index = __atomic_load_n(&control->object.index, __ATOMIC_ACQUIRE);
    if (!index) {
        emutls_init_once();
        while (__atomic_exchange_n(&emutls_index_lock, 1, __ATOMIC_ACQUIRE))
            sched_yield();
        index = control->object.index;
        if (!index) {
            index = ++emutls_num_object;
            __atomic_store_n(&control->object.index, index, __ATOMIC_RELEASE);
        }
        __atomic_store_n(&emutls_index_lock, 0, __ATOMIC_RELEASE);
    }
    return index;
}

/* Array size for `index`, rounded so header + data is a multiple of 16 words. */
static uintptr_t emutls_new_data_array_size(uintptr_t index) {
    uintptr_t header_words = sizeof(emutls_address_array) / sizeof(void *);
    return ((index + header_words + 15) & ~(uintptr_t)15) - header_words;
}

static size_t emutls_asize(uintptr_t n) {
    return n * sizeof(void *) + sizeof(emutls_address_array);
}

static emutls_address_array *emutls_get_address_array(uintptr_t index) {
    emutls_address_array *array = (emutls_address_array *)pthread_getspecific(emutls_key);
    if (array == 0) {
        uintptr_t new_size = emutls_new_data_array_size(index);
        array = (emutls_address_array *)malloc(emutls_asize(new_size));
        if (!array)
            abort();
        emutls_fill(array->data, 0, new_size * sizeof(void *));
        array->skip_destructor_rounds = EMUTLS_SKIP_DESTRUCTOR_ROUNDS;
        array->size = new_size;
        pthread_setspecific(emutls_key, array);
    } else if (index > array->size) {
        uintptr_t orig_size = array->size;
        uintptr_t new_size = emutls_new_data_array_size(index);
        array = (emutls_address_array *)realloc(array, emutls_asize(new_size));
        if (!array)
            abort();
        emutls_fill(array->data + orig_size, 0, (new_size - orig_size) * sizeof(void *));
        array->size = new_size;
        pthread_setspecific(emutls_key, array);
    }
    return array;
}

__attribute__((visibility("hidden"))) void *__emutls_get_address(__emutls_control *control) {
    uintptr_t index = emutls_get_index(control);
    emutls_address_array *array = emutls_get_address_array(index--);
    if (array->data[index] == 0) {
        size_t align = control->align;
        if (align < sizeof(void *))
            align = sizeof(void *);
        if ((align & (align - 1)) != 0)
            abort();
        void *base = emutls_memalign_alloc(align, control->size);
        emutls_fill(base, control->value, control->size);
        array->data[index] = base;
    }
    return array->data[index];
}
