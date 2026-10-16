#pragma once
#include "gnome.h"
#include <array>
#include <linux/input-event-codes.h>

namespace pab_linux {
// Existing wire keys are Windows VK values. Translate physical keys, preserving
// the target compositor's layout and modifier handling; never log typed data.
inline unsigned evdev_key(unsigned vk) {
    static constexpr unsigned letters[]{KEY_A,KEY_B,KEY_C,KEY_D,KEY_E,KEY_F,KEY_G,
        KEY_H,KEY_I,KEY_J,KEY_K,KEY_L,KEY_M,KEY_N,KEY_O,KEY_P,KEY_Q,KEY_R,KEY_S,
        KEY_T,KEY_U,KEY_V,KEY_W,KEY_X,KEY_Y,KEY_Z};
    static constexpr unsigned digits[]{KEY_0,KEY_1,KEY_2,KEY_3,KEY_4,KEY_5,KEY_6,KEY_7,KEY_8,KEY_9};
    static constexpr unsigned keypad[]{KEY_KP0,KEY_KP1,KEY_KP2,KEY_KP3,KEY_KP4,KEY_KP5,KEY_KP6,KEY_KP7,KEY_KP8,KEY_KP9};
    if (vk >= 0x41 && vk <= 0x5a) return letters[vk - 0x41];
    if (vk >= 0x30 && vk <= 0x39) return digits[vk - 0x30];
    if (vk >= 0x60 && vk <= 0x69) return keypad[vk - 0x60];
    if (vk >= 0x70 && vk <= 0x79) return KEY_F1 + vk - 0x70;
    if (vk >= 0x7a && vk <= 0x7b) return KEY_F11 + vk - 0x7a;
    if (vk >= 0x7c && vk <= 0x87) return KEY_F13 + vk - 0x7c;
    switch (vk) {
    case 0x08:return KEY_BACKSPACE; case 0x09:return KEY_TAB; case 0x0d:return KEY_ENTER;
    case 0x10:case 0xa0:return KEY_LEFTSHIFT; case 0xa1:return KEY_RIGHTSHIFT;
    case 0x11:case 0xa2:return KEY_LEFTCTRL; case 0xa3:return KEY_RIGHTCTRL;
    case 0x12:case 0xa4:return KEY_LEFTALT; case 0xa5:return KEY_RIGHTALT;
    case 0x13:return KEY_PAUSE; case 0x14:return KEY_CAPSLOCK; case 0x1b:return KEY_ESC;
    case 0x20:return KEY_SPACE; case 0x21:return KEY_PAGEUP; case 0x22:return KEY_PAGEDOWN;
    case 0x23:return KEY_END; case 0x24:return KEY_HOME; case 0x25:return KEY_LEFT;
    case 0x26:return KEY_UP; case 0x27:return KEY_RIGHT; case 0x28:return KEY_DOWN;
    case 0x2c:return KEY_SYSRQ; case 0x2d:return KEY_INSERT; case 0x2e:return KEY_DELETE;
    case 0x5b:return KEY_LEFTMETA; case 0x5c:return KEY_RIGHTMETA; case 0x5d:return KEY_COMPOSE;
    case 0x6a:return KEY_KPASTERISK; case 0x6b:return KEY_KPPLUS; case 0x6d:return KEY_KPMINUS;
    case 0x6e:return KEY_KPDOT; case 0x6f:return KEY_KPSLASH;
    case 0x90:return KEY_NUMLOCK; case 0x91:return KEY_SCROLLLOCK;
    case 0xba:return KEY_SEMICOLON; case 0xbb:return KEY_EQUAL; case 0xbc:return KEY_COMMA;
    case 0xbd:return KEY_MINUS; case 0xbe:return KEY_DOT; case 0xbf:return KEY_SLASH;
    case 0xc0:return KEY_GRAVE; case 0xdb:return KEY_LEFTBRACE; case 0xdc:return KEY_BACKSLASH;
    case 0xdd:return KEY_RIGHTBRACE; case 0xde:return KEY_APOSTROPHE; case 0xe2:return KEY_102ND;
    default:throw std::runtime_error("Unsupported remote desktop key");
    }
}
inline double stream_coordinate(uint16_t normalized, unsigned pixels) {
    if (pixels < 1 || pixels > 8192) throw std::runtime_error("Invalid input extent");
    return double(normalized) * double(pixels - 1) / 65535.0;
}
inline Variant property(GDBusConnection* bus, const char* service, const char* path,
                        const char* interface, const char* name) {
    auto result = call(bus, service, path, "org.freedesktop.DBus.Properties", "Get",
        g_variant_new("(ss)", interface, name), G_VARIANT_TYPE("(v)"));
    auto boxed = child(result.get(), 0);
    return Variant(g_variant_get_variant(boxed.get()));
}
class Input {
    GDBusConnection* bus_;
    Display display_;
    std::string session_, cast_, stream_;
    std::vector<guint> subscriptions_;
    std::array<bool, KEY_MAX + 1> keys_{};
    std::array<bool, 3> buttons_{};
    bool invalid_ = false, started_ = false;
    static constexpr const char* service = "org.gnome.Mutter.RemoteDesktop";
    static constexpr const char* interface = "org.gnome.Mutter.RemoteDesktop.Session";
    static constexpr const char* cast_service = "org.gnome.Mutter.ScreenCast";
    static void changed(GDBusConnection*, const gchar*, const gchar*, const gchar*, const gchar*, GVariant*, gpointer data) {
        static_cast<Input*>(data)->invalid_ = true;
    }
    void subscribe(const char* sender, const char* iface, const char* signal, const char* path, const char* arg = nullptr) {
        subscriptions_.push_back(g_dbus_connection_signal_subscribe(bus_, sender, iface, signal, path,
            arg, G_DBUS_SIGNAL_FLAGS_NONE, changed, this, nullptr));
    }
    void invoke(const char* method, GVariant* args = nullptr) {
        call(bus_, service, session_.c_str(), interface, method, args, G_VARIANT_TYPE("()"));
    }
    void valid() {
        pump();
        if (invalid_ || g_dbus_connection_is_closed(bus_)) throw std::runtime_error("Input screen or session changed");
    }
    void move(uint16_t x, uint16_t y) {
        invoke("NotifyPointerMotionAbsolute", g_variant_new("(sdd)", stream_.c_str(),
            stream_coordinate(x, display_.size.width), stream_coordinate(y, display_.size.height)));
    }
    void key(unsigned code, bool down) {
        if (keys_.at(code) == down) return;
        // Record downs before the call: a lost reply must still be released.
        if (down) keys_[code] = true;
        invoke("NotifyKeyboardKeycode", g_variant_new("(ub)", code, down));
        keys_[code] = down;
    }
    void button(unsigned index, bool down) {
        static constexpr int codes[]{BTN_LEFT, BTN_RIGHT, BTN_MIDDLE};
        if (buttons_.at(index) == down) return;
        if (down) buttons_[index] = true;
        invoke("NotifyPointerButton", g_variant_new("(ib)", codes[index], down));
        buttons_[index] = down;
    }
public:
    Input(GDBusConnection* bus, Display display) : bus_(bus), display_(std::move(display)) {}
    Input(const Input&) = delete;
    Input& operator=(const Input&) = delete;
    ~Input() {
        for (auto id : subscriptions_) g_dbus_connection_signal_unsubscribe(bus_, id);
        if (started_) { try { release(); } catch (...) {} }
        // Mutter owns the virtual devices and destroys them on Stop or peer
        // disappearance, including SIGKILL. Never release other local devices.
        if (!session_.empty()) { try { invoke("Stop"); } catch (...) {} }
    }
    void release() {
        for (unsigned i = 0; i < keys_.size(); ++i) if (keys_[i]) key(i, false);
        for (unsigned i = 0; i < buttons_.size(); ++i) if (buttons_[i]) button(i, false);
    }
    void start() {
        auto version = property(bus_, service, "/org/gnome/Mutter/RemoteDesktop", service, "Version");
        auto devices = property(bus_, service, "/org/gnome/Mutter/RemoteDesktop", service, "SupportedDeviceTypes");
        if (!g_variant_is_of_type(version.get(), G_VARIANT_TYPE_INT32) || g_variant_get_int32(version.get()) != 1 ||
            !g_variant_is_of_type(devices.get(), G_VARIANT_TYPE_UINT32) || (g_variant_get_uint32(devices.get()) & 3) != 3)
            throw std::runtime_error("Unsupported GNOME RemoteDesktop interface");
        subscribe("org.gnome.Mutter.DisplayConfig", "org.gnome.Mutter.DisplayConfig", "MonitorsChanged", "/org/gnome/Mutter/DisplayConfig");
        for (auto name : {service, cast_service})
            subscribe("org.freedesktop.DBus", "org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", name);
        if (displays(bus_).front().layout_serial != display_.layout_serial)
            throw std::runtime_error("Display layout changed before input startup");
        auto result = call(bus_, service, "/org/gnome/Mutter/RemoteDesktop", service, "CreateSession", nullptr, G_VARIANT_TYPE("(o)"));
        session_ = text(child(result.get(), 0).get());
        subscribe(service, interface, "Closed", session_.c_str());
        auto id = property(bus_, service, session_.c_str(), interface, "SessionId");
        if (!g_variant_is_of_type(id.get(), G_VARIANT_TYPE_STRING)) throw std::runtime_error("Invalid input session ID");
        GVariantBuilder options;
        g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
        g_variant_builder_add(&options, "{sv}", "remote-desktop-session-id", id.get());
        result = call(bus_, cast_service, "/org/gnome/Mutter/ScreenCast", cast_service, "CreateSession",
            g_variant_new("(a{sv})", &options), G_VARIANT_TYPE("(o)"));
        cast_ = text(child(result.get(), 0).get());
        result = call(bus_, cast_service, cast_.c_str(), "org.gnome.Mutter.ScreenCast.Session", "RecordMonitor",
            g_variant_new("(sa{sv})", display_.connector.c_str(), nullptr), G_VARIANT_TYPE("(o)"));
        stream_ = text(child(result.get(), 0).get());
        invoke("Start"); started_ = true; valid();
    }
    bool packet(std::array<uint8_t, 8>& bytes) {
        size_t offset = 0;
        while (offset < bytes.size()) {
            valid(); pollfd pipe{STDIN_FILENO, POLLIN, 0};
            auto status = poll(&pipe, 1, 20);
            if (status < 0 && errno == EINTR) continue;
            if (status < 0) throw std::runtime_error("Input pipe poll failed");
            if (!status) continue;
            auto count = read(STDIN_FILENO, bytes.data() + offset, bytes.size() - offset);
            if (count < 0 && errno == EINTR) continue;
            if (count < 0) throw std::runtime_error("Input pipe failed");
            if (!count) return false; // Also discards partial events, then releases.
            offset += size_t(count);
        }
        valid(); return true;
    }
    void apply(const std::array<uint8_t, 8>& bytes) {
        valid();
        const uint16_t x = bytes[1] | uint16_t(bytes[2]) << 8, y = bytes[3] | uint16_t(bytes[4]) << 8;
        const uint16_t value = bytes[5] | uint16_t(bytes[6]) << 8;
        if (bytes[7] > 1) throw std::runtime_error("Invalid input state");
        switch (bytes[0]) {
        case 1: if (value || bytes[7]) break; move(x, y); return;
        case 2: if (value > 2) break; move(x, y); button(value, bytes[7]); return;
        case 3:
            if (x || y || bytes[7]) break;
            // Existing wheel units are Windows WHEEL_DELTA (120 per notch).
            if (value) invoke("NotifyPointerAxis", g_variant_new("(ddu)", 0.0, -double(int16_t(value)) / 12.0, 3u));
            return;
        case 4: if (x || y) break; key(evdev_key(value), bytes[7]); return;
        case 5: if (x || y || value || bytes[7]) break; release(); return;
        }
        throw std::runtime_error("Invalid native input packet");
    }
};
inline void input(GDBusConnection* bus, const Display& display) {
    const auto x = read_value<int32_t>(), y = read_value<int32_t>();
    const auto width = read_value<uint32_t>(), height = read_value<uint32_t>();
    if (x != display.x || y != display.y || width != display.size.logical_width || height != display.size.logical_height)
        throw std::runtime_error("Selected input geometry changed");
    Input session(bus, display); session.start(); write_value(magic);
    std::array<uint8_t, 8> packet;
    while (session.packet(packet)) { session.apply(packet); write_value(uint8_t(1)); }
}
}
