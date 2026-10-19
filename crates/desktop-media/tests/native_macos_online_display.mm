// Deterministic CoreGraphics boundary regression. No capture permission,
// display-mode change, window or OS input is used.
#import <AppKit/AppKit.h>
#include <cassert>

static CGDirectDisplayID physical_display;
static bool connected = true, moved = false;
static unsigned active_queries = 0;
static CGError online_displays(uint32_t capacity, CGDirectDisplayID* ids, uint32_t* count) {
    assert(capacity >= 2);
    ids[0] = 42; ids[1] = 24; *count = 2;
    return kCGErrorSuccess;
}
static CGError active_displays(uint32_t, CGDirectDisplayID*, uint32_t* count) {
    ++active_queries; *count = 0; return kCGErrorSuccess;
}
static uint32_t is_active(CGDirectDisplayID) { ++active_queries; return 0; }
static uint32_t is_online(CGDirectDisplayID id) { return id == 24 || connected; }
static uint32_t is_main(CGDirectDisplayID id) { return id == 24; }
static CGRect test_display_bounds(CGDirectDisplayID id) {
    return id == 24 ? CGRectMake(0, 0, 1920, 1080) :
        CGRectMake(moved ? -1200 : -1280, 0, 1280, 720);
}
static CGDisplayModeRef test_display_mode(CGDirectDisplayID) { return CGDisplayCopyDisplayMode(physical_display); }

#define CGGetOnlineDisplayList online_displays
#define CGGetActiveDisplayList active_displays
#define CGDisplayIsActive is_active
#define CGDisplayIsOnline is_online
#define CGDisplayIsMain is_main
#define CGDisplayBounds test_display_bounds
#define CGDisplayCopyDisplayMode test_display_mode
#include "../native/macos/common.h"

int main() {
    @autoreleasepool {
        physical_display = CGMainDisplayID();
        auto found = pab_mac::displays();
        assert(found.size() == 2);
        assert(found[0].id == 42 && !found[0].primary && found[0].bounds.origin.x == -1280);
        assert(found[1].id == 24 && found[1].primary);
        assert(active_queries == 0);
        assert(pab_mac::display_current(found[0]));
        connected = false;
        assert(!pab_mac::display_current(found[0]));
        connected = true; moved = true;
        assert(!pab_mac::display_current(found[0]));
        moved = false;
        assert(pab_mac::display_current(found[0]));
        puts("{\"inactive_online_inventory\":true,\"primary_by_id\":true,\"unplug_rejected\":true,\"geometry_change_rejected\":true}");
    }
}
