#pragma once
#include <gio/gio.h>
#include <poll.h>
#include <unistd.h>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace pab_linux {
using Clock = std::chrono::steady_clock;
constexpr uint32_t magic = 0x31424150, max_packet = 16 * 1024 * 1024;
struct VariantDrop { void operator()(GVariant* v) const { if (v) g_variant_unref(v); } };
using Variant = std::unique_ptr<GVariant, VariantDrop>;
struct ObjectDrop { void operator()(GDBusConnection* v) const { if (v) g_object_unref(v); } };
using Connection = std::unique_ptr<GDBusConnection, ObjectDrop>;
inline Variant child(GVariant* v, size_t i) { return Variant(g_variant_get_child_value(v, i)); }
inline std::string text(GVariant* v) { return g_variant_get_string(v, nullptr); }
inline Variant call(GDBusConnection* bus, const char* service, const char* path,
                    const char* interface, const char* method, GVariant* args = nullptr,
                    const GVariantType* type = nullptr) {
    GError* error = nullptr;
    Variant result(g_dbus_connection_call_sync(bus, service, path, interface, method, args,
        type, G_DBUS_CALL_FLAGS_NONE, 3000, nullptr, &error));
    if (!result) {
        std::string message = std::string(method) + ": " + (error ? error->message : "D-Bus failed");
        if (error) g_error_free(error);
        throw std::runtime_error(message);
    }
    return result;
}
inline Connection session_bus() {
    GError* error = nullptr;
    Connection result(g_bus_get_sync(G_BUS_TYPE_SESSION, nullptr, &error));
    if (!result) {
        if (error) g_error_free(error);
        throw std::runtime_error("GNOME capture requires the active user's session bus");
    }
    return result;
}
inline void pump() {
    for (unsigned i = 0; i < 8 && g_main_context_pending(nullptr); ++i)
        g_main_context_iteration(nullptr, false);
}
inline void parent_alive() {
    pollfd pipe{STDIN_FILENO, POLLIN, 0};
    const auto status = poll(&pipe, 1, 0);
    if ((status < 0 && errno != EINTR) || (status > 0 && (pipe.revents & (POLLHUP | POLLERR | POLLNVAL))))
        throw std::runtime_error("Parent pipe ended");
}
inline void read_bytes(void* bytes, size_t size) {
    auto* out = static_cast<uint8_t*>(bytes);
    while (size) {
        pump(); pollfd pipe{STDIN_FILENO, POLLIN, 0};
        const auto status = poll(&pipe, 1, 10);
        if (status < 0 && errno == EINTR) continue;
        if (status < 0) throw std::runtime_error("Parent pipe poll failed");
        if (!status) continue;
        const auto count = read(STDIN_FILENO, out, size);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Parent pipe ended");
        out += count; size -= count;
    }
}
inline void write_bytes(const void* bytes, size_t size, bool check_parent = true) {
    const auto* in = static_cast<const uint8_t*>(bytes);
    const auto deadline = Clock::now() + std::chrono::seconds(5);
    while (size) {
        if (check_parent) parent_alive();
        pump(); pollfd pipe{STDOUT_FILENO, POLLOUT, 0};
        const auto status = poll(&pipe, 1, 10);
        if (status < 0 && errno == EINTR) continue;
        if (status < 0 || Clock::now() >= deadline) throw std::runtime_error("Media pipe write timed out");
        if (!status) continue;
        // A pipe's POLLOUT guarantees PIPE_BUF space, not a whole H.264 packet.
        const auto count = write(STDOUT_FILENO, in, std::min<size_t>(size, 4096));
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Media pipe write failed");
        in += count; size -= count;
    }
}
template<class T> T read_value() { T value{}; read_bytes(&value, sizeof(value)); return value; }
template<class T> void write_value(T value) { write_bytes(&value, sizeof(value)); }
inline unsigned number(const char* raw, unsigned limit) {
    std::string value(raw);
    if (value.empty() || value.size() > 9 || value.find_first_not_of("0123456789") != std::string::npos)
        throw std::runtime_error("Invalid number");
    auto n = std::stoul(value);
    if (!n || n > limit) throw std::runtime_error("Number outside bounds");
    return unsigned(n);
}
inline std::string json_string(const std::string& value) {
    if (!g_utf8_validate(value.data(), value.size(), nullptr)) throw std::runtime_error("Invalid display name");
    std::string out = "\"";
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') { out += '\\'; out += c; }
        else if (c < 32) { char escape[7]; snprintf(escape, sizeof(escape), "\\u%04x", c); out += escape; }
        else out += c;
    }
    return out + '"';
}
struct Geometry { unsigned width, height, logical_width, logical_height, scale_milli; };
inline Geometry geometry(int width, int height, double scale, unsigned transform, unsigned layout) {
    if (width < 2 || height < 2 || width > 8192 || height > 8192 ||
        uint64_t(width) * height > 33554432 || !std::isfinite(scale) || scale < .1 || scale > 8 ||
        transform > 7 || (layout != 1 && layout != 2)) throw std::runtime_error("Display geometry outside bounds");
    if (transform & 1) std::swap(width, height);
    auto lw = std::lround(layout == 1 ? width / scale : width);
    auto lh = std::lround(layout == 1 ? height / scale : height);
    if (lw < 1 || lh < 1 || lw > 8192 || lh > 8192) throw std::runtime_error("Logical geometry outside bounds");
    return {unsigned(width), unsigned(height), unsigned(lw), unsigned(lh), unsigned(std::lround(scale * 1000))};
}
struct Display {
    std::string connector, name;
    int x, y;
    Geometry size;
    bool primary;
    uint32_t layout_serial;
    std::string id() const { return "gnome:" + connector; }
};
}
