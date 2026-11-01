// WestPlay's macOS probe: a minimal SCS SDK telemetry plugin that proves where the Mac game loads plugins
// from, and reports what WestPlay's plugin will meet there (docs/MACOS_PORT.md, Phase 0). Not shipped.
//
// It logs through the SDK's log callback (lines start "[westplay-probe]" in game.log.txt): the process's
// architecture and whether Rosetta translates it, the graphics libraries loaded, and, from the first game
// frames, which OpenGL context is current on the thread the SDK's frame events come on.
//
// Build: tools/macos_probe/build.sh

#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <mach-o/dyld.h>
#include <sys/sysctl.h>
#include <unistd.h>
#include <OpenGL/OpenGL.h>
#include <pthread.h>

#include "scssdk_telemetry.h"
#include "eurotrucks2/scssdk_eut2.h"
#include "eurotrucks2/scssdk_telemetry_eut2.h"

static scs_log_t game_log = nullptr;
static unsigned frames = 0;

static void say(scs_log_type_t type, const char *format, ...) {
    if (!game_log) return;
    char line[1024];
    int prefix = snprintf(line, sizeof line, "[westplay-probe] ");
    va_list args;
    va_start(args, format);
    vsnprintf(line + prefix, sizeof line - prefix, format, args);
    va_end(args);
    game_log(type, line);
}

static void report_process() {
    int translated = 0;
    size_t size = sizeof translated;
    if (sysctlbyname("sysctl.proc_translated", &translated, &size, nullptr, 0) != 0) translated = -1;
#if defined(__x86_64__)
    const char *arch = "x86_64";
#elif defined(__aarch64__)
    const char *arch = "arm64";
#else
    const char *arch = "unknown";
#endif
    say(SCS_LOG_TYPE_message, "pid %d, plugin arch %s, Rosetta translated: %d", getpid(), arch, translated);
    for (uint32_t i = 0, count = _dyld_image_count(); i < count; i++) {
        const char *name = _dyld_get_image_name(i);
        if (strstr(name, "OpenGL") || strstr(name, "MoltenVK") || strstr(name, "Metal.framework/") ||
            strstr(name, "libGL") || strstr(name, "vulkan") || strstr(name, "/plugins/")) {
            say(SCS_LOG_TYPE_message, "image: %s", name);
        }
    }
}

static SCSAPI_VOID frame_end(const scs_event_t, const void *const, const scs_context_t) {
    frames++;
    if (frames == 1 || frames == 300) {
        CGLContextObj context = CGLGetCurrentContext();
        uint64_t thread = 0;
        pthread_threadid_np(nullptr, &thread);
        say(SCS_LOG_TYPE_message, "frame_end #%u on thread %llu (main: %d), current CGL context %p", frames,
            (unsigned long long)thread, pthread_main_np(), (void *)context);
        if (frames == 300) report_process();
    }
}

extern "C" __attribute__((visibility("default"))) SCSAPI_RESULT
scs_telemetry_init(const scs_u32_t version, const scs_telemetry_init_params_t *const params) {
    if (version != SCS_TELEMETRY_VERSION_1_01 && version != SCS_TELEMETRY_VERSION_1_00) {
        return SCS_RESULT_unsupported;
    }
    const auto *init = static_cast<const scs_telemetry_init_params_v100_t *>(params);
    game_log = init->common.log;
    say(SCS_LOG_TYPE_message, "loaded: %s (%s), game version %u.%u, api %u.%u", init->common.game_name,
        init->common.game_id, SCS_GET_MAJOR_VERSION(init->common.game_version),
        SCS_GET_MINOR_VERSION(init->common.game_version), SCS_GET_MAJOR_VERSION(version),
        SCS_GET_MINOR_VERSION(version));
    uint64_t thread = 0;
    pthread_threadid_np(nullptr, &thread);
    say(SCS_LOG_TYPE_message, "init on thread %llu (main: %d), current CGL context %p", (unsigned long long)thread,
        pthread_main_np(), (void *)CGLGetCurrentContext());
    report_process();
    init->register_for_event(SCS_TELEMETRY_EVENT_frame_end, frame_end, nullptr);
    return SCS_RESULT_ok;
}

extern "C" __attribute__((visibility("default"))) SCSAPI_VOID scs_telemetry_shutdown(void) {
    say(SCS_LOG_TYPE_message, "shutdown after %u frames", frames);
    game_log = nullptr;
}
