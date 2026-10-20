#pragma once
#include "x11_capture.h"
#include <X11/extensions/XRes.h>

namespace pab_linux::x11 {
// GetCursorImage describes the sprite even when XFixesHideCursor suppresses
// its display. Xorg keeps one XFixesCursorHideCount resource per hiding client
// and X screen until its last ShowCursor or disconnect. Query counts, never
// client names, window contents or input. RandR outputs share the same screen.
class CursorVisibility {
    Connection& connection_;
    Atom hide_type_ = None;
    bool available_ = false, hidden_ = false;
    Clock::time_point next_{};
public:
    explicit CursorVisibility(Connection& connection): connection_(connection) {
        auto* d = connection_.get();
        int event, error, major, minor;
        // XRes does not expose the screen owning a hide resource. Do not
        // misattribute another independent X screen's hide to this screen.
        available_ = ScreenCount(d) == 1 && XResQueryExtension(d, &event, &error) &&
            XResQueryVersion(d, &major, &minor) && major >= 1;
        if (available_) {
            hide_type_ = XInternAtom(d, "XFixesCursorHideCount", False);
            connection_.check();
        } else {
            fprintf(stderr, "media: X11 system cursor hiding cannot be observed on this server; "
                "shape and position remain available\n");
        }
    }
    bool hidden() {
        if (!available_ || Clock::now() < next_) return hidden_;
        auto* d = connection_.get();
        int count = 0; XResClient* raw_clients = nullptr;
        const auto ok = XResQueryClients(d, &count, &raw_clients);
        struct Free { void operator()(void* p) const { if (p) XFree(p); } };
        std::unique_ptr<XResClient, Free> clients(raw_clients);
        connection_.check();
        if (!ok || count < 0 || count > 4096 || (count && !clients))
            throw std::runtime_error("Cannot observe X11 cursor hide clients");
        bool hidden = false;
        for (int i = 0; i < count && !hidden; ++i) {
            int types = 0; XResType* raw_types = nullptr;
            const auto found = XResQueryClientResources(d, clients.get()[i].resource_base, &types, &raw_types);
            std::unique_ptr<XResType, Free> resources(raw_types);
            // A client may disconnect between enumeration and this request.
            // BadValue then means its hide resources have already been freed.
            const auto error = connection_.check(found ? 0 : BadValue);
            if (!found) {
                if (error == BadValue) continue;
                throw std::runtime_error("Cannot observe X11 cursor hide resources");
            }
            if (types < 0 || types > 4096 || (types && !resources))
                throw std::runtime_error("Invalid X11 cursor hide resources");
            for (int j = 0; j < types; ++j)
                hidden |= resources.get()[j].resource_type == hide_type_ && resources.get()[j].count != 0;
        }
        hidden_ = hidden;
        // Position still samples at 8 ms; resource inventory is independent
        // of frame rate and never causes an image conversion or video frame.
        next_ = Clock::now() + std::chrono::milliseconds(50);
        return hidden_;
    }
};
}
