#pragma once
#include "dma_broker.h"
#include "dma_capture.h"

namespace pab_linux {
inline virtual_input::Geometry input_geometry(GDBusConnection* bus, const Display& selected) {
    const auto found = displays(bus);
    int64_t left = INT32_MAX, top = INT32_MAX, right = INT32_MIN, bottom = INT32_MIN;
    bool present = false;
    for (const auto& screen : found) {
        if (screen.id() == selected.id() && screen.layout_serial == selected.layout_serial) present = true;
        left = std::min(left, int64_t(screen.x)); top = std::min(top, int64_t(screen.y));
        right = std::max(right, int64_t(screen.x) + screen.size.logical_width);
        bottom = std::max(bottom, int64_t(screen.y) + screen.size.logical_height);
    }
    if (!present || right - left > 262144 || bottom - top > 262144)
        throw std::runtime_error("Input desktop layout changed or exceeds bounds");
    virtual_input::Geometry result;
    result.left = int32_t(left); result.top = int32_t(top);
    result.width = int32_t(right - left); result.height = int32_t(bottom - top);
    result.x = selected.x; result.y = selected.y;
    result.screen_width = selected.size.logical_width; result.screen_height = selected.size.logical_height;
    virtual_input::validate(result); return result;
}
inline void supervised_input(GDBusConnection* bus, const Display& display) {
    dma::GnomeGuard guard(bus, display);
    const auto geometry = input_geometry(bus, display);
    auto channel = dma::connect_endpoint("input.sock");
    virtual_input::send(channel.get(), &geometry, sizeof(geometry));
    dma::await_ack(channel.get(), 7000);
    guard.check(); write_value(magic);
    std::array<uint8_t, 8> packet{}; size_t offset = 0;
    auto next_check = Clock::now();
    for (;;) {
        if (Clock::now() >= next_check) { guard.check(); next_check = Clock::now() + std::chrono::milliseconds(100); }
        pollfd input[2]{{STDIN_FILENO, POLLIN, 0}, {channel.get(), 0, 0}};
        auto result = poll(input, 2, 20);
        if (result < 0 && errno == EINTR) continue;
        if (result < 0 || input[1].revents) throw std::runtime_error("Supervised input ended");
        if (!input[0].revents) continue;
        const auto count = read(STDIN_FILENO, packet.data() + offset, packet.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count < 0) throw std::runtime_error("Native input pipe failed");
        if (!count) return;
        offset += size_t(count);
        if (offset != packet.size()) continue;
        guard.check();
        virtual_input::send(channel.get(), packet.data(), packet.size());
        dma::await_ack(channel.get(), 1000);
        write_value(uint8_t(1)); offset = 0;
    }
}
}
