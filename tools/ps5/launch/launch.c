/*
 * ps5-homebrew-dev-protocol - Title-aware application launcher.
 * Copyright (C) 2026 BlackBearReloaded
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Expects BOOTSTRAP_TITLE_ID to be defined at compile time, for example:
 *   -DBOOTSTRAP_TITLE_ID="\"PPSA99900\""
 *
 * From UnleashedRecomp-Prospero tools/controllers/launch.c; Frogger 2 port: the result
 * also goes to klog (sceKernelDebugOutText) and a notification. Built per title by
 * tools/ps5/build-hw-test.sh as an elfldr payload (send to port 9021).
 */

#include <stdint.h>
#include <stdio.h>
#include <string.h>

#ifndef BOOTSTRAP_TITLE_ID
#error "BOOTSTRAP_TITLE_ID must be defined"
#endif

typedef struct {
    uint32_t size;
    int32_t user_id;
    uint32_t app_opt;
    uint64_t crash_report;
    uint32_t check_flag;
} LaunchAppParam;

int sceUserServiceInitialize(void *);
int sceUserServiceGetForegroundUser(int32_t *);
int sceSystemServiceLaunchApp(const char *, const char **, LaunchAppParam *);
int sceKernelDebugOutText(int, const char *);
int sceKernelSendNotificationRequest(int, const void *, size_t, int);

static void report(const char *msg) {
    static struct {
        char reserved[45];
        char message[3075];
    } req;
    char line[256];
    snprintf(line, sizeof(line), "[f2launch] %s\n", msg);
    sceKernelDebugOutText(0, line);
    memset(&req, 0, sizeof(req));
    strncpy(req.message, msg, sizeof(req.message) - 1);
    sceKernelSendNotificationRequest(0, &req, sizeof(req), 0);
}

int main(void) {
    LaunchAppParam app_param;
    const char *argv[] = { NULL };
    int32_t user_id = -1;
    int rc;

    memset(&app_param, 0, sizeof(app_param));
    app_param.size = sizeof(LaunchAppParam);

    rc = sceUserServiceInitialize(NULL);
    if (rc == 0) {
        sceUserServiceGetForegroundUser(&user_id);
    }
    app_param.user_id = user_id;

    printf("Bootstrap launch requested for %s (user_id=0x%08x)\n",
           BOOTSTRAP_TITLE_ID, (unsigned int)user_id);
    rc = sceSystemServiceLaunchApp(BOOTSTRAP_TITLE_ID, argv, &app_param);
    printf("sceSystemServiceLaunchApp(%s) returned 0x%08x\n",
           BOOTSTRAP_TITLE_ID, (unsigned int)rc);
    {
        char msg[128];
        snprintf(msg, sizeof(msg), "sceSystemServiceLaunchApp(%s) = 0x%08x",
                 BOOTSTRAP_TITLE_ID, (unsigned int)rc);
        report(msg);
    }

    return (rc < 0) ? 1 : 0;
}
