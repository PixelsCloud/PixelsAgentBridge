#pragma once
#include "capture.h"
#include "dma_image.h"

namespace pab_linux::dma {
inline bool protected_screen(GDBusConnection* bus) {
    // GDM has no org.gnome.ScreenSaver. The supervisor supplies the logind
    // class; this only selects a backend, never authorizes a privileged broker.
    // Broker peer/session/compositor checks still apply to every connection.
    const char* session_class = std::getenv("XDG_SESSION_CLASS");
    if (session_class && std::strcmp(session_class, "greeter") == 0) return true;
    auto value = call(bus, "org.gnome.ScreenSaver", "/org/gnome/ScreenSaver",
        "org.gnome.ScreenSaver", "GetActive", nullptr, G_VARIANT_TYPE("(b)"));
    return g_variant_get_boolean(child(value.get(), 0).get());
}
inline bool asleep(GDBusConnection* bus) {
    auto result = call(bus, "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
        "org.freedesktop.DBus.Properties", "Get",
        g_variant_new("(ss)", "org.gnome.Mutter.DisplayConfig", "PowerSaveMode"), G_VARIANT_TYPE("(v)"));
    auto boxed = child(result.get(), 0); Variant power(g_variant_get_variant(boxed.get()));
    if (!g_variant_is_of_type(power.get(), G_VARIANT_TYPE_INT32)) throw std::runtime_error("Display power state unavailable");
    // Mutter uses -1 for unsupported/unknown. Do not turn that into a wake
    // request or divert an otherwise usable ordinary PipeWire session.
    const auto mode = g_variant_get_int32(power.get());
    return mode >= 1 && mode <= 3;
}
inline void await_awake(GDBusConnection* bus) {
    const auto deadline = Clock::now() + std::chrono::seconds(3);
    while (asleep(bus)) {
        parent_alive(); pump();
        if (Clock::now() >= deadline) throw std::runtime_error("Display did not wake");
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    // Power-on is asynchronous with the compositor's first visible repaint.
    std::this_thread::sleep_for(std::chrono::milliseconds(300));
    parent_alive();
}
// The GNOME lock/login screen normally blanks after a short idle timeout.
// Keep its idle watches inhibited only for this protected capture's lifetime;
// ordinary viewing retains the user's normal automatic lock behavior.
class LockedScreenAwake {
    GDBusConnection* bus_;
    uint32_t cookie_ = 0;
    bool held_ = false;
public:
    explicit LockedScreenAwake(GDBusConnection* bus) : bus_(bus) {
        if (!protected_screen(bus_)) return;
        try {
            auto result = call(bus_, "org.gnome.SessionManager", "/org/gnome/SessionManager",
                "org.gnome.SessionManager", "Inhibit",
                g_variant_new("(susu)", "Pixels Agent Bridge", 0u,
                    "Viewing the locked remote desktop", 8u), G_VARIANT_TYPE("(u)"));
            cookie_ = g_variant_get_uint32(child(result.get(), 0).get());
            held_ = true;
            fprintf(stderr, "media: locked-screen idle inhibition acquired\n");
        } catch (const std::exception& error) {
            // An unsupported session manager must not hide an otherwise
            // available frame; the existing stale-scanout guard still applies.
            fprintf(stderr, "media: locked-screen idle inhibition unavailable: %s\n", error.what());
        }
    }
    LockedScreenAwake(const LockedScreenAwake&) = delete;
    LockedScreenAwake& operator=(const LockedScreenAwake&) = delete;
    ~LockedScreenAwake() {
        if (!held_) return;
        try {
            call(bus_, "org.gnome.SessionManager", "/org/gnome/SessionManager",
                "org.gnome.SessionManager", "Uninhibit", g_variant_new("(u)", cookie_));
        } catch (...) {
            // GNOME also retires this connection's cookie when the dedicated
            // capture worker exits, including SIGKILL or a broken parent pipe.
            fprintf(stderr, "media: releasing locked-screen inhibition on worker exit\n");
        }
    }
};
class GnomeGuard {
    GDBusConnection* bus_;
    Display display_;
    bool locked_;
    bool locked() {
        return protected_screen(bus_);
    }
public:
    GnomeGuard(GDBusConnection* bus, Display display) : bus_(bus), display_(std::move(display)), locked_(locked()) { check(); }
    void check() {
        auto result = call(bus_, "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
            "org.freedesktop.DBus.Properties", "Get",
            g_variant_new("(ss)", "org.gnome.Mutter.DisplayConfig", "PowerSaveMode"), G_VARIANT_TYPE("(v)"));
        auto boxed = child(result.get(), 0); Variant power(g_variant_get_variant(boxed.get()));
        if (!g_variant_is_of_type(power.get(), G_VARIANT_TYPE_INT32) || g_variant_get_int32(power.get()) != 0)
            throw std::runtime_error("Display is asleep; refusing stale scanout");
        if (locked() != locked_) throw std::runtime_error("Lock state changed; restart capture generation");
        const auto found = displays(bus_);
        const auto current = std::find_if(found.begin(), found.end(), [&](const Display& d) { return d.id() == display_.id(); });
        if (current == found.end() || current->layout_serial != display_.layout_serial ||
            current->size.width != display_.size.width || current->size.height != display_.size.height)
            throw std::runtime_error("DMA display layout changed");
    }
};
inline void capture(GDBusConnection* bus, const Display& display, unsigned fps, int socket) {
    if (geteuid() == 0) throw std::runtime_error("DMA conversion must run as the graphical user");
    channel(socket);
    const PabVideoGeometry geometry(display.size.width, display.size.height);
    GnomeGuard guard(bus, display); LockedScreenAwake awake(bus); ImageReader reader;
    Encoder encoder(pab_soft_encoder_new(display.size.width, display.size.height, fps));
    if (!encoder) throw std::runtime_error("Cannot initialize DMA software encoder");
    write_value(magic); write_value(display.size.width); write_value(display.size.height);
    const auto start = Clock::now(); auto next = start; uint64_t sequence = 0;
    for (;;) {
        const auto command = read_value<uint8_t>();
        if (!command) return;
        if (command != 1) throw std::runtime_error("Invalid DMA capture command");
        while (Clock::now() < next) { parent_alive(); pump(); std::this_thread::sleep_for(std::chrono::milliseconds(1)); }
        guard.check(); send_request(socket, sequence);
        // Parent closure/cancellation retires this worker, including an export
        // in flight. Keep the receive bounded independently of network traffic.
        const auto deadline = Clock::now() + std::chrono::seconds(3);
        for (;;) {
            parent_alive(); pollfd wait{socket, POLLIN, 0};
            const auto status = poll(&wait, 1, 10);
            if (status > 0) break;
            if ((status < 0 && errno != EINTR) || Clock::now() >= deadline)
                throw std::runtime_error("DMA export response timed out");
        }
        auto frame = receive(socket, sequence);
        if (frame.info.width != display.size.width || frame.info.height != display.size.height)
            throw std::runtime_error("DMA frame dimensions changed");
        const auto& pixels = reader.read(frame); const auto sampled = Clock::now();
        guard.check();
        PabCodecBytes packet{};
        if (!pab_soft_encode(encoder.get(), pixels.data(), pixels.size(), size_t(display.size.width) * 4, &packet) ||
            !packet.size || packet.size > max_packet) throw std::runtime_error("DMA software encoding failed");
        guard.check(); // Never publish an old desktop after a lock/power transition during encoding.
        const bool key = keyframe(packet);
        if (!sequence && !key) throw std::runtime_error("First DMA frame is not IDR");
        if (!sequence) fprintf(stderr, "media source=drm_dma encoder=openh264_software direct_bgra=%s\n", reader.direct_bgra() ? "true" : "false");
        write_value(uint32_t(packet.size)); write_value(sequence++);
        write_value(uint64_t(std::chrono::duration_cast<std::chrono::microseconds>(sampled - start).count()));
        write_value(uint32_t(key)); write_bytes(packet.data, packet.size);
        next = std::max(next + std::chrono::microseconds(1000000 / fps), Clock::now());
    }
}
}
