/*
 * Emulator-only shims for running vktri in KytyPS5 (tools/ps5/build-vktri.sh, KYTY_SHIM=1).
 * KytyPS5 2026-10-10 has no HLE for libSceLibcInternal getenv (calls land in its
 * "unresolved import" stub, which returns 0) and its strdup returns NULL, so Mesa aborts in
 * xmlconfig ("out of memory"). These wrappers keep a title-local environment and duplicate
 * strings on the title heap. Not for the console. SPDX-License-Identifier: MIT OR Apache-2.0
 */
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/types.h>

#define MAX_ENV 32
static char env_names[MAX_ENV][64];
static char env_values[MAX_ENV][128];
static int env_count;

__attribute__((visibility("hidden"))) char *__wrap_getenv(const char *name)
{
   for (int i = 0; i < env_count; i++)
      if (strcmp(env_names[i], name) == 0)
         return env_values[i];
   return NULL;
}

__attribute__((visibility("hidden"))) int __wrap_setenv(const char *name, const char *value, int overwrite)
{
   for (int i = 0; i < env_count; i++)
      if (strcmp(env_names[i], name) == 0) {
         if (overwrite)
            strncpy(env_values[i], value, sizeof(env_values[i]) - 1);
         return 0;
      }
   if (env_count == MAX_ENV)
      return -1;
   strncpy(env_names[env_count], name, sizeof(env_names[0]) - 1);
   strncpy(env_values[env_count], value, sizeof(env_values[0]) - 1);
   env_count++;
   return 0;
}

/* KytyPS5's strdup returns NULL here; route it through malloc (the platform heap). */
void *malloc(size_t);
__attribute__((visibility("hidden"))) char *__wrap_strdup(const char *s)
{
   size_t n = strlen(s) + 1;
   char *d = malloc(n);
   if (d)
      memcpy(d, s, n);
   return d;
}

__attribute__((visibility("hidden"))) char *__wrap_strndup(const char *s, size_t max)
{
   size_t n = strnlen(s, max);
   char *d = malloc(n + 1);
   if (d) {
      memcpy(d, s, n);
      d[n] = 0;
   }
   return d;
}
