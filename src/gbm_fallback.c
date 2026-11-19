#define _GNU_SOURCE

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

struct gbm_device;
typedef struct gbm_device *(*gbm_create_device_fn)(int fd);

static unsigned long g_calls;
static unsigned long g_denied;

static void log_line(const char *fmt, ...)
{
    char buffer[1024];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buffer, sizeof(buffer), fmt, ap);
    va_end(ap);
    if (n <= 0)
        return;
    size_t len = (size_t)n < sizeof(buffer) ? (size_t)n : sizeof(buffer) - 1;
    (void)write(STDERR_FILENO, buffer, len);
}

static bool read_cmdline(char *buffer, size_t capacity, size_t *length)
{
    int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return false;
    ssize_t n = read(fd, buffer, capacity - 1);
    int saved_errno = errno;
    close(fd);
    errno = saved_errno;
    if (n <= 0)
        return false;
    buffer[n] = '\0';
    *length = (size_t)n;
    return true;
}

static bool cmdline_has_pair(const char *cmdline, size_t length,
                             const char *option, const char *value)
{
    size_t option_len = strlen(option);
    size_t value_len = strlen(value);
    size_t pos = 0;
    while (pos < length) {
        const char *arg = cmdline + pos;
        size_t remaining = length - pos;
        size_t arg_len = strnlen(arg, remaining);
        if (arg_len == option_len + 1 + value_len &&
            memcmp(arg, option, option_len) == 0 && arg[option_len] == '=' &&
            memcmp(arg + option_len + 1, value, value_len) == 0)
            return true;
        if (arg_len == option_len && memcmp(arg, option, option_len) == 0) {
            size_t next_pos = pos + arg_len + 1;
            if (next_pos < length) {
                const char *next_arg = cmdline + next_pos;
                size_t next_len = strnlen(next_arg, length - next_pos);
                if (next_len == value_len && memcmp(next_arg, value, value_len) == 0)
                    return true;
            }
        }
        pos += arg_len + 1;
    }
    return false;
}

static bool is_chromium_wayland_target(const char *target)
{
    char exe[512];
    ssize_t exe_len = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
    if (exe_len <= 0)
        return false;
    exe[exe_len] = '\0';
    if (!strstr(exe, "chromium"))
        return false;

    char cmdline[16384];
    size_t cmdline_len = 0;
    if (!read_cmdline(cmdline, sizeof(cmdline), &cmdline_len))
        return false;

    return cmdline_has_pair(cmdline, cmdline_len, "--ozone-platform", "wayland") &&
           cmdline_has_pair(cmdline, cmdline_len, "--render-node-override", target);
}

static bool fd_matches_target_node(int fd, const char *target)
{
    struct stat target_stat;
    struct stat fd_stat;

    /* Fail closed: only match the exact configured character device. */
    if (stat(target, &target_stat) != 0 ||
        fstat(fd, &fd_stat) != 0 ||
        !S_ISCHR(target_stat.st_mode) ||
        !S_ISCHR(fd_stat.st_mode)) {
        return false;
    }

    return target_stat.st_rdev == fd_stat.st_rdev;
}

static gbm_create_device_fn resolve_real(void)
{
    dlerror();
    void *symbol = dlsym(RTLD_NEXT, "gbm_create_device");
    if (!symbol) {
        const char *error = dlerror();
        log_line("[igpu-linear-bridge] dlsym(gbm_create_device) failed: %s\n",
                 error ? error : "unknown error");
        return NULL;
    }
    gbm_create_device_fn function = NULL;
    _Static_assert(sizeof(function) == sizeof(symbol), "function/data pointer size mismatch");
    memcpy(&function, &symbol, sizeof(function));
    return function;
}

__attribute__((constructor))
static void bridge_init(void)
{
    const char *enabled = getenv("IGPU_GBM_FALLBACK_ENABLE");
    if (enabled && strcmp(enabled, "1") == 0) {
        const char *target = getenv("IGPU_GBM_FALLBACK_TARGET");
        if (!target || !*target)
            target = "/dev/dri/renderD129";
        const char *mode = getenv("IGPU_GBM_FALLBACK_MODE");
        if (!mode || !*mode)
            mode = "observe";
        log_line("[igpu-linear-bridge] loaded: enable=1 mode=%s target=%s pid=%ld\n",
                 mode, target, (long)getpid());
    }
}

__attribute__((visibility("default")))
struct gbm_device *gbm_create_device(int fd)
{
    static _Atomic(gbm_create_device_fn) real_fn;
    gbm_create_device_fn real = atomic_load_explicit(&real_fn, memory_order_acquire);
    if (!real) {
        gbm_create_device_fn resolved = resolve_real();
        if (resolved) {
            gbm_create_device_fn expected = NULL;
            if (!atomic_compare_exchange_strong_explicit(
                    &real_fn, &expected, resolved,
                    memory_order_release, memory_order_acquire)) {
                real = expected;
            } else {
                real = resolved;
            }
        }
    }
    if (!real) {
        errno = ENOSYS;
        return NULL;
    }

    const char *enabled = getenv("IGPU_GBM_FALLBACK_ENABLE");
    if (!enabled || strcmp(enabled, "1") != 0)
        return real(fd);

    const char *target = getenv("IGPU_GBM_FALLBACK_TARGET");
    if (!target || !*target)
        target = "/dev/dri/renderD129";

    if (!is_chromium_wayland_target(target) || !fd_matches_target_node(fd, target))
        return real(fd);

    unsigned long call = __atomic_add_fetch(&g_calls, 1, __ATOMIC_RELAXED);
    const char *mode = getenv("IGPU_GBM_FALLBACK_MODE");
    if (!mode || !*mode)
        mode = "observe";

    Dl_info caller_info = {0};
    (void)dladdr(__builtin_return_address(0), &caller_info);
    const char *caller = caller_info.dli_fname ? caller_info.dli_fname : "<unknown>";

    if (strcmp(mode, "deny") == 0) {
        __atomic_add_fetch(&g_denied, 1, __ATOMIC_RELAXED);
        if (call <= 64) {
            log_line("[igpu-linear-bridge] gbm_create_device #%lu fd=%d target=%s caller=%s -> NULL (ENODEV)\n",
                     call, fd, target, caller);
        }
        errno = ENODEV;
        return NULL;
    }

    if (call <= 64) {
        log_line("[igpu-linear-bridge] gbm_create_device #%lu fd=%d target=%s caller=%s -> passthrough (mode=%s)\n",
                 call, fd, target, caller, mode);
    }
    return real(fd);
}

__attribute__((destructor))
static void bridge_fini(void)
{
    unsigned long calls = __atomic_load_n(&g_calls, __ATOMIC_RELAXED);
    unsigned long denied = __atomic_load_n(&g_denied, __ATOMIC_RELAXED);
    if (calls || denied)
        log_line("[igpu-linear-bridge] summary: matched_calls=%lu denied=%lu\n",
                 calls, denied);
}
