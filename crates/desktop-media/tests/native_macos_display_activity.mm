// OS calls are mocked: this regression never wakes or locks the real display.
#import <IOKit/pwr_mgt/IOPMLib.h>
#include <cassert>
#include <cstdio>
#include <type_traits>
#include <vector>

static int failure = 0;
static unsigned creates = 0, wakes = 0;
static std::vector<IOPMAssertionID> released;
static IOReturn test_create(CFStringRef kind, IOPMAssertionLevel level,
        CFStringRef, IOPMAssertionID* out) {
    ++creates;
    assert(CFEqual(kind, kIOPMAssertPreventUserIdleDisplaySleep));
    assert(level == kIOPMAssertionLevelOn);
    if (failure == 1) return kIOReturnError;
    *out = 71;
    return kIOReturnSuccess;
}
static IOReturn test_wake(CFStringRef, IOPMUserActiveType type, IOPMAssertionID* out) {
    ++wakes;
    assert(type == kIOPMUserActiveLocal);
    if (failure == 2) return kIOReturnError;
    *out = 72;
    return kIOReturnSuccess;
}
static IOReturn test_release(IOPMAssertionID id) {
    released.push_back(id);
    return kIOReturnSuccess;
}
#define IOPMAssertionCreateWithName test_create
#define IOPMAssertionDeclareUserActivity test_wake
#define IOPMAssertionRelease test_release
#include "../native/macos/display_activity.h"

int main() {
    static_assert(!std::is_copy_constructible_v<pab_mac::CaptureDisplayActivity>);
    {
        pab_mac::CaptureDisplayActivity active;
        assert(creates == 1 && wakes == 1 && released.empty());
    }
    assert((released == std::vector<IOPMAssertionID>{72, 71}));
    for (failure = 1; failure <= 2; ++failure) {
        released.clear();
        bool rejected = false;
        try { pab_mac::CaptureDisplayActivity active; }
        catch (const std::runtime_error&) { rejected = true; }
        assert(rejected);
        if (failure == 1) assert(released.empty());
        else assert((released == std::vector<IOPMAssertionID>{71}));
    }
    puts("{\"capture_scope_release\":true,\"create_failure_cleanup\":true,\"wake_failure_cleanup\":true}");
}
