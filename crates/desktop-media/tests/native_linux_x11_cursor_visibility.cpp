#include "../native/linux/x11_cursor.h"
#include <X11/cursorfont.h>
#include <cassert>
#include <cstdio>

// Only run on the disposable server started by the acceptance driver.
int main(int argc, char** argv) {
    using namespace pab_linux;
    assert(argc == 2 && std::string(argv[1]) == "--isolated-display");
    x11::Connection connection;
    auto* d = connection.get();
    assert(ScreenCount(d) == 1);
    const auto outputs = x11::outputs(connection);
    assert(!outputs.empty());
    const auto root = outputs.front().root;
    const auto arrow = XCreateFontCursor(d, XC_left_ptr);
    XDefineCursor(d, root, arrow);
    XWarpPointer(d, None, root, 0, 0, 0, 0, 100, 100);
    connection.check();
    auto open = [&] {
        auto* client = XOpenDisplay(nullptr); assert(client);
        int major = 5, minor = 0;
        assert(XFixesQueryVersion(client, &major, &minor));
        return client;
    };
    auto* a = open(); auto* b = open();
    auto hide = [&](::Display* client) { XFixesHideCursor(client, root); XSync(client, False); };
    auto show = [&](::Display* client) { XFixesShowCursor(client, root); XSync(client, False); };
    x11::CursorSource source(connection, outputs.front());
    source.sample(); const auto initial = source.state.snapshot();
    assert(initial.visible && initial.image);
    auto expect = [&](bool visible) {
        std::this_thread::sleep_for(std::chrono::milliseconds(60));
        source.sample(); const auto value = source.state.snapshot();
        assert(value.visible == visible);
        assert(value.image == initial.image && value.image_id == initial.image_id);
        assert(value.x == 100 && value.y == 100);
    };
    hide(a); expect(false);
    // Starting after the hide must work without observing a request/event.
    {
        x11::CursorSource late(connection, outputs.front());
        late.sample(); assert(!late.state.snapshot().visible);
    }
    hide(a); expect(false);
    show(a); expect(false);
    hide(b); expect(false);
    show(a); expect(false);
    XCloseDisplay(b); b = nullptr; expect(true);
    // A disconnected client in the inventory is normal, not a cursor failure.
    for (unsigned i = 0; i < 12; ++i) {
        auto* transient = open(); hide(transient);
        XCloseDisplay(transient); expect(true);
    }
    // Stable samples must remain cached between visibility polls.
    const auto before = source.state.snapshot();
    const auto begin = Clock::now();
    for (unsigned i = 0; i < 100; ++i) source.sample();
    const auto elapsed = std::chrono::duration<double, std::milli>(Clock::now() - begin).count();
    const auto after = source.state.snapshot();
    assert(before.revision == after.revision && before.image == after.image);
    XCloseDisplay(a);
    XUndefineCursor(d, root); XFreeCursor(d, arrow); connection.check();
    printf("{\"hide_detected\":true,\"startup_hidden\":true,\"nested_hide\":true,"
        "\"second_client_preserved\":true,\"disconnect_restored\":true,"
        "\"disconnect_cycles\":12,\"shape_reused\":true,\"stable_samples\":100,"
        "\"stable_samples_ms\":%.3f}\n", elapsed);
}
