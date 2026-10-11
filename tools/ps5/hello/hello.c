/* PS5 hardware test ladder, step 1: a minimal C title packaged exactly like PS5_Vulkan's
 * hardware-validated titles (ps5-native-app-boilerplate: its app_crt.cpp start-up, the PS5
 * Clang target with -femulated-tls, its SDK fork, ps5-pie.ld, the unpatched ps5-native-tool
 * and libc.prx). If this does not start, the packaging / loader path is the problem, not
 * the Rust port. Built by tools/ps5/build.sh --hello (title PPSA99922).
 *
 * It writes "[f2hello]" lines to the kernel log (klog) and stdout, posts a notification,
 * tries to write a file in the places the game uses, then shows a solid colour that
 * cycles green -> blue -> white every two seconds (so a frozen picture is obvious) and
 * keeps running until closed from the home screen.
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>

int sceKernelDebugOutText(int channel, const char *text);
int sceKernelSendNotificationRequest(int device, const void *req, size_t size, int blocking);
int sceKernelAllocateDirectMemory(int64_t start, int64_t end, size_t len, size_t align, int type, int64_t *phys);
size_t sceKernelGetDirectMemorySize(void);
int sceKernelMapDirectMemory(void **addr, size_t len, int prot, int flags, int64_t phys, size_t align);
int sceKernelUsleep(unsigned int us);
int sceKernelOpen(const char *path, int flags, int mode);
long sceKernelWrite(int fd, const void *buf, size_t n);
int sceKernelClose(int fd);
int sceKernelMkdir(const char *path, int mode);
int sceVideoOutOpen(int user, int bus, int index, const void *param);
int sceVideoOutSetFlipRate(int handle, int rate);
void sceVideoOutSetBufferAttribute2(void *attr, uint64_t format, uint32_t tiling, uint32_t width, uint32_t height, uint64_t option,
                                    uint32_t dcc, uint64_t dcc_clear);
int sceVideoOutRegisterBuffers2(int handle, int set, int start, const void *buffers, int count, const void *attr, int category,
                                const void *option);
int sceVideoOutSubmitFlip(int handle, int index, uint32_t mode, int64_t arg);
int sceVideoOutWaitVblank(int handle);
int sceSystemServiceHideSplashScreen(void);

static void say(const char *msg) {
    char line[512];
    snprintf(line, sizeof line, "[f2hello] %s\n", msg);
    sceKernelDebugOutText(0, line);
    printf("%s", line);
}

static void notify(const char *msg) {
    /* The payload SDK's notify_request_t: 45 reserved bytes, then the text. */
    static struct {
        char reserved[45];
        char message[3075];
    } req;
    memset(&req, 0, sizeof req);
    strncpy(req.message, msg, sizeof req.message - 1);
    sceKernelSendNotificationRequest(0, &req, sizeof req, 0);
}

int main(void) {
    char buf[256];
    say("main entered (C hello, PS5_Vulkan packaging)");
    notify("Frogger 2 HW test 1: C hello is running");

    static const char *const paths[] = {"/data/frogger2/hello.txt", "/user/data/frogger2/hello.txt", "/data/hello-f2.txt",
                                        "/download0/hello.txt", "/user/temp/hello.txt"};
    sceKernelMkdir("/data/frogger2", 0777);
    sceKernelMkdir("/user/data/frogger2", 0777);
    for (size_t i = 0; i < sizeof paths / sizeof paths[0]; i++) {
        static const char text[] = "Frogger 2 HW test 1 (C hello) ran\n";
        /* O_WRONLY | O_CREAT | O_TRUNC */
        int fd = sceKernelOpen(paths[i], 0x1 | 0x200 | 0x400, 0666);
        long w = fd >= 0 ? sceKernelWrite(fd, text, sizeof text - 1) : fd;
        if (fd >= 0)
            sceKernelClose(fd);
        snprintf(buf, sizeof buf, "write %s: %s (%#lx)", paths[i], w == (long)(sizeof text - 1) ? "ok" : "failed", (unsigned long)w);
        say(buf);
    }

    int h = sceVideoOutOpen(0xff, 0, 0, NULL);
    snprintf(buf, sizeof buf, "sceVideoOutOpen: %#x", h);
    say(buf);
    if (h < 0)
        goto idle;
    /* Two 1920x1080 RGBA8 buffers (tiling 0: the scan-out layout; a solid colour does not
     * care about the tile order). 1920x1080 rounds up to whole 64 KiB tiles. */
    const uint32_t w = 1920, hgt = 1080;
    const size_t fb = 0x800000, align = 0x200000;
    int64_t phys = 0;
    int r = sceKernelAllocateDirectMemory(0, (int64_t)sceKernelGetDirectMemorySize(), fb * 2, align, 3, &phys);
    void *mem = NULL;
    if (r == 0)
        r = sceKernelMapDirectMemory(&mem, fb * 2, 0x33, 0, phys, align);
    snprintf(buf, sizeof buf, "direct memory: %#x", r);
    say(buf);
    if (r != 0)
        goto idle;
    struct {
        void *data, *metadata, *reserved[2];
    } bufs[2] = {{mem, NULL, {NULL, NULL}}, {(char *)mem + fb, NULL, {NULL, NULL}}};
    _Alignas(16) uint8_t attr[80] = {0};
    sceVideoOutSetBufferAttribute2(attr, 0x8000000022000000ull, 0, w, hgt, 0, 0, 0);
    r = sceVideoOutRegisterBuffers2(h, 0, 0, bufs, 2, attr, 0, NULL);
    snprintf(buf, sizeof buf, "sceVideoOutRegisterBuffers2: %#x", r);
    say(buf);
    if (r != 0)
        goto idle;
    sceVideoOutSetFlipRate(h, 0);
    sceSystemServiceHideSplashScreen();
    static const uint32_t colours[] = {0xff20c040u, 0xffc06020u, 0xffffffffu}; /* ABGR: green, blue, white */
    for (int64_t frame = 1;; frame++) {
        uint32_t *p = (uint32_t *)bufs[frame & 1].data;
        uint32_t c = colours[(frame / 120) % 3];
        for (size_t i = 0; i < fb / 4; i++)
            p[i] = c;
        sceVideoOutSubmitFlip(h, (int)(frame & 1), 1, frame);
        sceVideoOutWaitVblank(h);
        if (frame % 600 == 0) {
            snprintf(buf, sizeof buf, "frame %lld", (long long)frame);
            say(buf);
        }
    }
idle:
    for (;;) {
        say("idle (no video)");
        sceKernelUsleep(5000000);
    }
}
