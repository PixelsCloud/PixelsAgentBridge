#pragma once
#include "common.h"

namespace pab_linux {
// Mutter's versioned private adapter. This does not claim generic Wayland,
// X11, a greeter, or a pre-boot display. No persistent compositor settings.
inline std::vector<Display> displays(GDBusConnection* bus) {
    auto state = call(bus, "org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
        "org.gnome.Mutter.DisplayConfig", "GetCurrentState", nullptr,
        G_VARIANT_TYPE("(ua((ssss)a(siiddada{sv})a{sv})a(iiduba(ssss)a{sv})a{sv})"));
    auto physical = child(state.get(), 1), logical = child(state.get(), 2), properties = child(state.get(), 3);
    if (g_variant_n_children(physical.get()) > 32 || g_variant_n_children(logical.get()) > 32)
        throw std::runtime_error("Display count outside bounds");
    uint32_t layout = 1;
    g_variant_lookup(properties.get(), "layout-mode", "u", &layout);
    std::vector<Display> result;
    bool primary_assigned = false;
    for (size_t i = 0; i < g_variant_n_children(logical.get()); ++i) {
        auto group = child(logical.get(), i);
        const int x = g_variant_get_int32(child(group.get(), 0).get()), y = g_variant_get_int32(child(group.get(), 1).get());
        const double scale = g_variant_get_double(child(group.get(), 2).get());
        const auto transform = g_variant_get_uint32(child(group.get(), 3).get());
        const bool primary = g_variant_get_boolean(child(group.get(), 4).get());
        auto specs = child(group.get(), 5);
        if (!g_variant_n_children(specs.get()) || g_variant_n_children(specs.get()) > 32)
            throw std::runtime_error("Invalid logical display group");
        for (size_t j = 0; j < g_variant_n_children(specs.get()); ++j) {
            auto spec = child(specs.get(), j);
            auto connector = text(child(spec.get(), 0).get());
            if (connector.empty() || connector.size() > 120) throw std::runtime_error("Invalid display connector");
            bool found = false;
            for (size_t k = 0; k < g_variant_n_children(physical.get()); ++k) {
                auto monitor = child(physical.get(), k), candidate = child(monitor.get(), 0);
                if (!g_variant_equal(spec.get(), candidate.get())) continue;
                auto modes = child(monitor.get(), 1), props = child(monitor.get(), 2);
                if (g_variant_n_children(modes.get()) > 1024) throw std::runtime_error("Display modes outside bounds");
                const char* name = nullptr;
                g_variant_lookup(props.get(), "display-name", "&s", &name);
                for (size_t m = 0; m < g_variant_n_children(modes.get()); ++m) {
                    auto mode = child(modes.get(), m), flags = child(mode.get(), 6);
                    gboolean active = false;
                    if (!g_variant_lookup(flags.get(), "is-current", "b", &active) || !active) continue;
                    if (found) throw std::runtime_error("Multiple active display modes");
                    auto size = geometry(g_variant_get_int32(child(mode.get(), 1).get()),
                        g_variant_get_int32(child(mode.get(), 2).get()), scale, transform, layout);
                    if (int64_t(x) + size.logical_width > INT32_MAX || int64_t(y) + size.logical_height > INT32_MAX)
                        throw std::runtime_error("Display origin outside bounds");
                    for (const auto& d : result) if (d.connector == connector) throw std::runtime_error("Duplicate display connector");
                    result.push_back({connector, name ? name : connector, x, y, size, primary && !primary_assigned,
                        g_variant_get_uint32(child(state.get(), 0).get())});
                    if (primary) primary_assigned = true;
                    found = true;
                }
            }
            if (!found) throw std::runtime_error("Active display mode unavailable");
        }
    }
    if (result.empty() || result.size() > 32) throw std::runtime_error("No bounded active GNOME displays");
    if (!primary_assigned) result.front().primary = true;
    return result;
}
inline void inventory(const std::vector<Display>& found) {
    std::string json = "[";
    for (const auto& d : found) {
        if (json.size() > 1) json += ',';
        json += "{\"id\":" + json_string(d.id()) + ",\"name\":" + json_string(d.name) +
            ",\"x\":" + std::to_string(d.x) + ",\"y\":" + std::to_string(d.y) +
            ",\"desktop_width\":" + std::to_string(d.size.logical_width) + ",\"desktop_height\":" + std::to_string(d.size.logical_height) +
            ",\"pixel_width\":" + std::to_string(d.size.width) + ",\"pixel_height\":" + std::to_string(d.size.height) +
            ",\"scale_milli\":" + std::to_string(d.size.scale_milli) + ",\"rotation\":\"identity\",\"primary\":" + (d.primary ? "true}" : "false}");
    }
    // PipeWire monitor frames are already oriented by the compositor.
    json += "]\n"; write_bytes(json.data(), json.size(), false);
}
}
