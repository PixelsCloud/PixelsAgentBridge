// Diagnostic only. Run on a disposable two-screen Xvfb server, never a user's
// display: the fixture hides the cursor on each root and opens two clients.
// XRes counts are deliberately NOT wired into the production cursor source.
#include <X11/Xlib.h>
#include <X11/extensions/Xfixes.h>
#include <X11/extensions/XRes.h>
#include <cassert>
#include <cstdio>
#include <cstring>

struct Observation {
    unsigned hides = 0;
    unsigned resources = 0;
    long cross_references = 0;
};

static Observation observe(Display* observer, const char* phase) {
    Observation result;
    int count = 0;
    XResClient* clients = nullptr;
    assert(XResQueryClients(observer, &count, &clients));
    // Obtain the atom after querying resources: XRes may intern names lazily.
    for (int i = 0; i < count; ++i) {
        int types_count = 0;
        XResType* types = nullptr;
        assert(XResQueryClientResources(observer, clients[i].resource_base, &types_count, &types));
        Atom hidden = None;
        for (int j = 0; j < types_count; ++j) {
            char* name = XGetAtomName(observer, types[j].resource_type);
            assert(name);
            if (std::strcmp(name, "XFixesCursorHideCount") == 0) {
                hidden = types[j].resource_type;
                result.hides += types[j].count;
            }
            XFree(name);
        }
        XFree(types);
        if (hidden != None) {
            XResResourceIdSpec spec{};
            long size_count = 0;
            XResResourceSizeValue* sizes = nullptr;
            // Unlike the v1.0 queries above, the v1.2 API returns Success (0).
            assert(XResQueryResourceBytes(observer, clients[i].resource_base, 1, &spec, &size_count, &sizes) == Success);
            for (long j = 0; j < size_count; ++j) {
                if (sizes[j].size.spec.type == hidden) {
                    ++result.resources;
                    result.cross_references += sizes[j].num_cross_references;
                }
            }
            XResResourceSizeValuesDestroy(size_count, sizes);
        }
    }
    XFree(clients);
    std::printf("{\"phase\":\"%s\",\"hide_resources\":%u,\"size_resources\":%u,\"cross_references\":%ld}\n",
                phase, result.hides, result.resources, result.cross_references);
    return result;
}

int main(int argc, char** argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    assert(argc == 2 && std::strcmp(argv[1], "--isolated-display") == 0);
    Display* observer = XOpenDisplay(nullptr);
    Display* first = XOpenDisplay(nullptr);
    Display* second = XOpenDisplay(nullptr);
    assert(observer && first && second && ScreenCount(observer) == 2);
    int major = 1, minor = 2;
    assert(XResQueryVersion(observer, &major, &minor));
    assert(major > 1 || (major == 1 && minor >= 2));
    std::printf("{\"xres_major\":%d,\"xres_minor\":%d,\"x_screens\":2}\n", major, minor);
    auto expect = [&](const char* phase, unsigned n) {
        const auto result = observe(observer, phase);
        assert(result.hides == n && result.resources == n);
        // No root/window cross-reference is exported for these resources.
        assert(result.cross_references == 0);
    };
    auto hide = [](Display* d, int screen) { XFixesHideCursor(d, RootWindow(d, screen)); XSync(d, False); };
    auto show = [](Display* d, int screen) { XFixesShowCursor(d, RootWindow(d, screen)); XSync(d, False); };
    expect("initial", 0);
    hide(first, 0); expect("first_hides_screen0", 1);
    hide(first, 0); expect("nested_hide_screen0", 1);
    show(first, 0); expect("nested_show_still_hidden", 1);
    hide(second, 0); expect("second_client_hides_screen0", 2);
    show(first, 0); expect("first_released_second_still_hides", 1);
    show(second, 0); expect("all_shown", 0);
    hide(first, 1); expect("first_hides_only_screen1", 1);
    hide(first, 0); expect("first_hides_both_screens", 2);
    XCloseDisplay(first);
    XSync(observer, False);
    expect("disconnect_releases_both", 0);
    XCloseDisplay(second);
    XCloseDisplay(observer);
    std::puts("{\"nested_hide_and_disconnect_observable\":true,\"screen_attribution_available\":false,\"production_visibility_fix\":false}");
}
