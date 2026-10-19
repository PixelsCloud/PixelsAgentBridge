#pragma once
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <VideoToolbox/VideoToolbox.h>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include <poll.h>
#include <unistd.h>

namespace pab_mac {
using Clock = std::chrono::steady_clock;
constexpr uint32_t magic = 0x31424150;
constexpr uint32_t max_packet = 16 * 1024 * 1024;

template<class T> class OwnedCF {
    T value_ = nullptr;
public:
    explicit OwnedCF(T value = nullptr) : value_(value) {}
    ~OwnedCF() { if (value_) CFRelease(value_); }
    OwnedCF(const OwnedCF&) = delete;
    OwnedCF& operator=(const OwnedCF&) = delete;
    OwnedCF(OwnedCF&& other) noexcept : value_(other.release()) {}
    OwnedCF& operator=(OwnedCF&& other) noexcept {
        if (this != &other) { if (value_) CFRelease(value_); value_ = other.release(); }
        return *this;
    }
    T get() const { return value_; }
    T release() { T value = value_; value_ = nullptr; return value; }
};
inline void check(OSStatus status, const char* operation) {
    if (status != noErr)
        throw std::runtime_error(std::string(operation) + " failed (" + std::to_string(status) + ")");
}
inline unsigned number(const char* value, unsigned maximum) {
    const std::string text(value);
    if (text.empty() || text.find_first_not_of("0123456789") != std::string::npos)
        throw std::runtime_error("Invalid number");
    size_t end = 0;
    const auto result = std::stoul(text, &end);
    if (!result || end != text.size() || result > maximum) throw std::runtime_error("Number outside bounds");
    return static_cast<unsigned>(result);
}
inline void pump() {
    const auto next = Clock::now() + std::chrono::milliseconds(2);
    @autoreleasepool { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.002, true); }
    // A headless worker's run loop can return immediately with no sources.
    // Keep static-screen and completion waits from becoming a CPU busy loop.
    std::this_thread::sleep_until(next);
}
inline void parent_alive() {
    pollfd pipe{STDIN_FILENO, POLLIN, 0};
    const int status = poll(&pipe, 1, 0);
    if (status < 0 && errno != EINTR) throw std::runtime_error("Parent pipe poll failed");
    if (status > 0 && (pipe.revents & (POLLHUP | POLLERR | POLLNVAL)))
        throw std::runtime_error("Parent pipe ended");
}
inline void read_bytes(void* target, size_t size, const std::function<void()>& idle = {}) {
    auto* out = static_cast<uint8_t*>(target);
    while (size) {
        pollfd pipe{STDIN_FILENO, POLLIN, 0};
        const int status = poll(&pipe, 1, 2);
        if (status < 0 && errno == EINTR) continue;
        if (status < 0) throw std::runtime_error("Parent pipe poll failed");
        // Do not sleep/pump once for every already-buffered field. Pipe
        // framing otherwise adds several run-loop deadlines to each frame.
        if (!status) { if (idle) idle(); pump(); continue; }
        const auto count = read(STDIN_FILENO, out, size);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Parent pipe ended");
        out += count; size -= count;
    }
}
inline void write_bytes(const void* bytes, size_t size) {
    const auto* in = static_cast<const uint8_t*>(bytes);
    while (size) {
        const auto count = write(STDOUT_FILENO, in, size);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Parent pipe write failed");
        in += count; size -= count;
    }
}
template<class T> T read_value(const std::function<void()>& idle = {}) { T value{}; read_bytes(&value, sizeof(value), idle); return value; }
template<class T> void write_value(T value) { write_bytes(&value, sizeof(value)); }

struct Display {
    CGDirectDisplayID id;
    CGRect bounds;
    unsigned width, height, scale_milli;
    bool primary;
};
inline bool display_current(const Display& display) {
    // A locked or sleeping Aqua session can report an online display as
    // inactive. Only disconnection or changed geometry retires this layout.
    return CGDisplayIsOnline(display.id) &&
        CGRectEqualToRect(CGDisplayBounds(display.id), display.bounds);
}
inline std::vector<Display> displays() {
    CGDirectDisplayID ids[33]; uint32_t count = 0;
    check(CGGetOnlineDisplayList(33, ids, &count), "Online displays");
    if (!count || count > 32) throw std::runtime_error("Display count outside bounds");
    std::vector<Display> result;
    for (uint32_t i = 0; i < count; ++i) {
        const CGRect bounds = CGDisplayBounds(ids[i]);
        OwnedCF<CGDisplayModeRef> mode(CGDisplayCopyDisplayMode(ids[i]));
        if (!mode.get() || !CGDisplayModeGetWidth(mode.get())) throw std::runtime_error("Display mode unavailable");
        const double scale = double(CGDisplayModeGetPixelWidth(mode.get())) / CGDisplayModeGetWidth(mode.get());
        const double w = std::round(bounds.size.width * scale), h = std::round(bounds.size.height * scale);
        if (!std::isfinite(scale) || scale < 0.1 || scale > 8 ||
            !std::isfinite(w) || !std::isfinite(h) || w < 2 || h < 2 || w > 8192 || h > 8192 || w * h > 33554432 ||
            !std::isfinite(bounds.origin.x) || !std::isfinite(bounds.origin.y) ||
            bounds.origin.x < INT32_MIN || bounds.origin.y < INT32_MIN ||
            bounds.origin.x + bounds.size.width > INT32_MAX || bounds.origin.y + bounds.size.height > INT32_MAX ||
            bounds.size.width < 1 || bounds.size.height < 1 || bounds.size.width > 8192 || bounds.size.height > 8192)
            throw std::runtime_error("Display geometry outside bounds");
        // ScreenCaptureKit delivers the image already oriented. Its texture
        // therefore has identity rotation even for a physically rotated screen.
        result.push_back({ids[i], bounds, unsigned(w), unsigned(h), unsigned(std::lround(scale * 1000)), CGDisplayIsMain(ids[i]) != 0});
    }
    return result;
}
inline void inventory(const std::vector<Display>& found) {
    NSMutableArray* entries = [NSMutableArray array];
    for (const auto& d : found) {
        NSString* name = [NSString stringWithFormat:@"Display %u", d.id];
        for (NSScreen* screen in NSScreen.screens)
            if ([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue] == d.id) { name = screen.localizedName; break; }
        [entries addObject:@{@"id": [NSString stringWithFormat:@"%u", d.id], @"name": name,
            @"x": @(std::lround(d.bounds.origin.x)), @"y": @(std::lround(d.bounds.origin.y)),
            @"desktop_width": @(std::lround(d.bounds.size.width)), @"desktop_height": @(std::lround(d.bounds.size.height)),
            @"pixel_width": @(d.width), @"pixel_height": @(d.height), @"scale_milli": @(d.scale_milli),
            @"rotation": @"identity", @"primary": @(d.primary)}];
    }
    NSError* error = nil;
    NSData* json = [NSJSONSerialization dataWithJSONObject:entries options:0 error:&error];
    if (!json) throw std::runtime_error("Display inventory serialization failed");
    write_bytes(json.bytes, json.length);
    write_bytes("\n", 1);
}
}
