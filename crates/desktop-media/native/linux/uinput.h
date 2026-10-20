#pragma once
// Owned per input connection. Device removal cannot affect physical devices;
// normal teardown additionally emits releases before destroying our devices.
#include "dma_frame.h"
#include "keycodes.h"
#include <dirent.h>
#include <linux/uinput.h>
#include <csignal>
#include <sys/ioctl.h>

namespace pab_linux::virtual_input {
inline volatile sig_atomic_t stopping = 0;
inline void terminate(int) { stopping = 1; }
struct Geometry {
    uint32_t magic = 0x31494e50, version = 1;
    int32_t left = 0, top = 0, width = 0, height = 0;
    int32_t x = 0, y = 0, screen_width = 0, screen_height = 0;
};
static_assert(sizeof(Geometry) == 40);
inline void validate(const Geometry& g) {
    if (g.magic != 0x31494e50 || g.version != 1 || g.width < 2 || g.height < 2 ||
        g.width > 262144 || g.height > 262144 || g.screen_width < 1 || g.screen_height < 1 ||
        g.screen_width > 8192 || g.screen_height > 8192 || g.x < g.left || g.y < g.top ||
        int64_t(g.left) + g.width > INT32_MAX || int64_t(g.top) + g.height > INT32_MAX ||
        int64_t(g.x) + g.screen_width > int64_t(g.left) + g.width ||
        int64_t(g.y) + g.screen_height > int64_t(g.top) + g.height)
        throw std::runtime_error("Invalid supervised input geometry");
}
inline int coordinate(uint16_t normalized, int origin, int extent) {
    return int(int64_t(origin) + (int64_t(normalized) * (extent - 1) + 32767) / 65535);
}
inline void alive(int process, int socket, bool allow_eof = false) {
    pollfd wait[2]{{process, POLLIN, 0}, {socket, 0, 0}};
    if (stopping || poll(wait, 2, 0) < 0 || wait[0].revents ||
        (wait[1].revents & (POLLERR | POLLNVAL)) || (!allow_eof && (wait[1].revents & POLLHUP)))
        throw std::runtime_error("Supervised input session ended");
}
inline void send(int socket, const void* data, size_t size) {
    if (::send(socket, data, size, MSG_NOSIGNAL) != ssize_t(size))
        throw std::runtime_error("Supervised input response failed");
}
inline bool receive(int process, int socket, void* data, size_t size, bool initial = false) {
    const auto deadline = Clock::now() + std::chrono::seconds(3);
    for (;;) {
        alive(process, socket, true);
        pollfd pending{socket, POLLIN, 0};
        const auto result = poll(&pending, 1, 20);
        if (result < 0 && errno == EINTR) continue;
        if (result < 0) throw std::runtime_error("Input channel poll failed");
        if (!result) {
            if (initial && Clock::now() >= deadline) throw std::runtime_error("Input setup timed out");
            continue;
        }
        const auto count = recv(socket, data, size, MSG_TRUNC);
        if (!count) return false;
        if (count != ssize_t(size)) throw std::runtime_error("Invalid supervised input packet length");
        return true;
    }
}

class Device {
    dma::Fd fd_;
    bool created_ = false;
    std::array<bool, KEY_MAX + 1> held_{};
    void option(unsigned long request, unsigned value) {
        if (ioctl(fd_.get(), request, value)) throw std::runtime_error("Cannot configure virtual input device");
    }
public:
    explicit Device(bool keyboard, const Geometry& geometry = {})
        : fd_(open("/dev/uinput", O_WRONLY | O_NONBLOCK | O_CLOEXEC | O_NOFOLLOW)) {
        if (geteuid() != 0 || fd_.get() < 0) throw std::runtime_error("System input service cannot open uinput");
        option(UI_SET_EVBIT, EV_KEY);
        if (keyboard) {
            std::array<bool, KEY_MAX + 1> supported{};
            for (unsigned vk = 1; vk < 256; ++vk) {
                try { supported.at(evdev_key(vk)) = true; } catch (const std::runtime_error&) {}
            }
            for (unsigned key = 1; key < supported.size(); ++key) if (supported[key]) option(UI_SET_KEYBIT, key);
        } else {
            validate(geometry);
            for (unsigned key : {BTN_LEFT, BTN_RIGHT, BTN_MIDDLE}) option(UI_SET_KEYBIT, key);
            option(UI_SET_EVBIT, EV_ABS); option(UI_SET_EVBIT, EV_REL);
            for (unsigned code : {REL_X, REL_Y, REL_WHEEL}) option(UI_SET_RELBIT, code);
            for (unsigned code : {ABS_X, ABS_Y}) {
                option(UI_SET_ABSBIT, code);
                uinput_abs_setup axis{}; axis.code = code;
                axis.absinfo.minimum = code == ABS_X ? geometry.left : geometry.top;
                axis.absinfo.maximum = axis.absinfo.minimum + (code == ABS_X ? geometry.width : geometry.height) - 1;
                if (ioctl(fd_.get(), UI_ABS_SETUP, &axis)) throw std::runtime_error("Cannot configure absolute pointer geometry");
            }
        }
        uinput_setup setup{}; setup.id.bustype = BUS_USB;
        setup.id.vendor = 0x1209; setup.id.product = keyboard ? 1 : 2; setup.id.version = 1;
        snprintf(setup.name, sizeof(setup.name), "%s", keyboard ? "Pixels Remote Keyboard" : "Pixels Remote Pointer");
        if (ioctl(fd_.get(), UI_DEV_SETUP, &setup) || ioctl(fd_.get(), UI_DEV_CREATE))
            throw std::runtime_error("Cannot create virtual input device");
        created_ = true;
    }
    Device(const Device&) = delete;
    Device& operator=(const Device&) = delete;
    ~Device() {
        if (!created_) return;
        try { release(); } catch (...) {}
        if (ioctl(fd_.get(), UI_DEV_DESTROY)) fprintf(stderr, "media: virtual input device teardown failed\n");
    }
    void emit(unsigned type, unsigned code, int value) {
        input_event event{}; event.type = type; event.code = code; event.value = value;
        const auto deadline = Clock::now() + std::chrono::milliseconds(200);
        for (;;) {
            const auto count = write(fd_.get(), &event, sizeof(event));
            if (count == sizeof(event)) return;
            if (count < 0 && errno == EINTR) continue;
            if (count < 0 && errno == EAGAIN && Clock::now() < deadline) {
                pollfd pending{fd_.get(), POLLOUT, 0}; poll(&pending, 1, 10); continue;
            }
            throw std::runtime_error("Virtual input event failed");
        }
    }
    void sync() { emit(EV_SYN, SYN_REPORT, 0); }
    void key(unsigned code, bool down) {
        if (held_.at(code) == down) return;
        if (down) held_[code] = true;
        emit(EV_KEY, code, down); sync(); held_[code] = down;
    }
    void release() {
        for (unsigned code = 0; code < held_.size(); ++code) if (held_[code]) key(code, false);
    }
    // Wait for the actual Wayland compositor to open this device, rather than
    // guessing a sleep after UI_DEV_CREATE and losing the first login key.
    void adopted(unsigned pid, int process, int socket) {
        char name[64]{};
        if (ioctl(fd_.get(), UI_GET_SYSNAME(sizeof(name)), name) < 0 ||
            strncmp(name, "input", 5) || !memchr(name, 0, sizeof(name)))
            throw std::runtime_error("Cannot locate virtual input device");
        const std::string sys = std::string("/sys/devices/virtual/input/") + name;
        const auto deadline = Clock::now() + std::chrono::seconds(3);
        struct Close { void operator()(DIR* d) const { if (d) closedir(d); } };
        for (;;) {
            alive(process, socket);
            std::vector<dev_t> nodes;
            std::unique_ptr<DIR, Close> directory(opendir(sys.c_str()));
            if (directory) while (auto entry = readdir(directory.get())) {
                const std::string event(entry->d_name);
                if (event.rfind("event", 0) || event.size() <= 5 || event.substr(5).find_first_not_of("0123456789") != std::string::npos) continue;
                struct stat info{};
                if (!stat(("/dev/input/" + event).c_str(), &info) && S_ISCHR(info.st_mode)) nodes.push_back(info.st_rdev);
            }
            const auto path = std::string("/proc/") + std::to_string(pid) + "/fd";
            std::unique_ptr<DIR, Close> descriptors(opendir(path.c_str()));
            if (descriptors) while (auto entry = readdir(descriptors.get())) {
                struct stat info{};
                if (!stat((path + "/" + entry->d_name).c_str(), &info) && S_ISCHR(info.st_mode) &&
                    std::find(nodes.begin(), nodes.end(), info.st_rdev) != nodes.end()) return;
            }
            if (Clock::now() >= deadline) throw std::runtime_error("Compositor did not adopt the virtual input device");
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
        }
    }
};

inline void wake(unsigned pid, int process, int socket) {
    Device keyboard(true); keyboard.adopted(pid, process, socket);
    keyboard.key(KEY_LEFTSHIFT, true);
    std::this_thread::sleep_for(std::chrono::milliseconds(40));
    keyboard.key(KEY_LEFTSHIFT, false);
    // Let the compositor process both events before removing the device.
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
}
inline void serve(unsigned pid, int process, int socket) {
    if (geteuid() != 0 || pid <= 1 || process < 3 || socket < 3)
        throw std::runtime_error("Virtual input requires the session supervisor");
    stopping = 0; signal(SIGTERM, terminate); signal(SIGINT, terminate);
    dma::channel(socket);
    Geometry geometry;
    if (!receive(process, socket, &geometry, sizeof(geometry), true)) return;
    validate(geometry);
    Device keyboard(true), pointer(false, geometry);
    keyboard.adopted(pid, process, socket); pointer.adopted(pid, process, socket);
    const uint8_t ack = 1; send(socket, &ack, sizeof(ack));
    std::array<uint8_t, 8> packet{}; int wheel = 0;
    while (receive(process, socket, packet.data(), packet.size())) {
        const unsigned x = packet[1] | unsigned(packet[2]) << 8, y = packet[3] | unsigned(packet[4]) << 8;
        const unsigned value = packet[5] | unsigned(packet[6]) << 8;
        if (packet[7] > 1) throw std::runtime_error("Invalid input state");
        auto move = [&] {
            pointer.emit(EV_ABS, ABS_X, coordinate(x, geometry.x, geometry.screen_width));
            pointer.emit(EV_ABS, ABS_Y, coordinate(y, geometry.y, geometry.screen_height)); pointer.sync();
        };
        switch (packet[0]) {
        case 1:
            if (value || packet[7]) throw std::runtime_error("Invalid pointer packet");
            move(); break;
        case 2: {
            if (value > 2) throw std::runtime_error("Invalid button packet");
            static constexpr unsigned buttons[]{BTN_LEFT, BTN_RIGHT, BTN_MIDDLE};
            move(); pointer.key(buttons[value], packet[7]); break;
        }
        case 3:
            if (x || y || packet[7]) throw std::runtime_error("Invalid wheel packet");
            wheel += int16_t(value);
            if (wheel / 120) { pointer.emit(EV_REL, REL_WHEEL, wheel / 120); pointer.sync(); }
            wheel %= 120; break;
        case 4:
            if (x || y) throw std::runtime_error("Invalid key packet");
            keyboard.key(evdev_key(value), packet[7]); break;
        case 5:
            if (x || y || value || packet[7]) throw std::runtime_error("Invalid release packet");
            keyboard.release(); pointer.release(); wheel = 0; break;
        default: throw std::runtime_error("Invalid native input operation");
        }
        send(socket, &ack, sizeof(ack));
    }
}
}
