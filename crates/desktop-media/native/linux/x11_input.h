#pragma once
#include "x11_capture.h"
#include "input.h"
#include <X11/extensions/XTest.h>
#include <fcntl.h>
#include <sys/file.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <fstream>
#include <sstream>

namespace pab_linux::x11 {
inline std::string server_generation(::Display* display) {
    // The local transport's kernel peer, not DISPLAY or a claimed remote PID,
    // distinguishes X server restarts. /proc starttime prevents PID reuse.
    struct ucred peer{}; socklen_t size = sizeof(peer);
    if (getsockopt(ConnectionNumber(display), SOL_SOCKET, SO_PEERCRED, &peer, &size) ||
        size != sizeof(peer) || peer.pid <= 0) throw std::runtime_error("Local X server identity unavailable");
    std::ifstream file("/proc/" + std::to_string(peer.pid) + "/stat");
    std::string line; std::getline(file, line);
    const auto end = line.rfind(')');
    if (line.size() > 8192 || end == std::string::npos) throw std::runtime_error("X server lifetime unavailable");
    std::istringstream fields(line.substr(end + 1)); std::string value;
    for (unsigned i = 0; i < 20; ++i) if (!(fields >> value)) throw std::runtime_error("Invalid X server lifetime");
    if (value.empty() || value.size() > 20 || value.find_first_not_of("0123456789") != std::string::npos)
        throw std::runtime_error("Invalid X server starttime");
    return std::to_string(peer.pid) + "-" + value;
}
class Releases {
    int file_ = -1;
    // Runtime state only: outstanding X keycodes and logical buttons 1..5.
    // No text, event history, credentials or application configuration.
    std::array<uint8_t, 261> flags_{};
    static void owned_directory(int fd) {
        struct stat info{};
        if (fd < 0 || fstat(fd, &info) || !S_ISDIR(info.st_mode) || info.st_uid != getuid() || (info.st_mode & 077))
            throw std::runtime_error("Input runtime directory is not private to the session user");
    }
public:
    explicit Releases(::Display* display) {
        const char* runtime = std::getenv("XDG_RUNTIME_DIR");
        if (!runtime || *runtime != '/') throw std::runtime_error("X11 input requires the user's runtime directory");
        const int parent = open(runtime, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        int directory = -1;
        try {
            owned_directory(parent);
            if (mkdirat(parent, "pixels-remote-input", 0700) && errno != EEXIST)
                throw std::runtime_error("Cannot create input recovery directory");
            directory = openat(parent, "pixels-remote-input", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
            owned_directory(directory);
            const auto name = "x11-" + server_generation(display) + ".releases";
            file_ = openat(directory, name.c_str(), O_RDWR | O_CREAT | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, 0600);
            struct stat info{};
            if (file_ < 0 || fstat(file_, &info) || !S_ISREG(info.st_mode) || info.st_uid != getuid() ||
                (info.st_mode & 077) || info.st_nlink != 1 || flock(file_, LOCK_EX | LOCK_NB))
                throw std::runtime_error("Input recovery state is busy or invalid");
            if (info.st_size == 0) {
                // ftruncate supplies zero bytes even if killed during startup.
                if (ftruncate(file_, flags_.size())) throw std::runtime_error("Cannot initialize input recovery state");
            } else if (info.st_size != off_t(flags_.size())) throw std::runtime_error("Invalid input recovery size");
            if (pread(file_, flags_.data(), flags_.size(), 0) != ssize_t(flags_.size()))
                throw std::runtime_error("Cannot read input recovery state");
            for (auto flag : flags_) if (flag > 1) throw std::runtime_error("Invalid input recovery flag");
            close(directory); close(parent);
        } catch (...) {
            if (file_ >= 0) close(file_);
            file_ = -1;
            if (directory >= 0) close(directory);
            if (parent >= 0) close(parent);
            throw;
        }
    }
    ~Releases() { if (file_ >= 0) close(file_); }
    Releases(const Releases&) = delete;
    bool held(size_t index) const { return flags_.at(index) != 0; }
    void save(size_t index, bool down) {
        if (held(index) == down) return;
        const uint8_t value = down;
        ssize_t written;
        do { written = pwrite(file_, &value, 1, index); } while (written < 0 && errno == EINTR);
        if (written != 1) throw std::runtime_error("Cannot persist input release state");
        // This is per-login runtime storage. Successful pwrite survives process
        // death; a machine reboot destroys the matching X server's input state.
        flags_[index] = value;
    }
};
inline void extension(Connection& connection) {
    int event, error, major, minor;
    if (!XTestQueryExtension(connection.get(), &event, &error, &major, &minor))
        throw std::runtime_error("X11 XTEST input extension unavailable");
    connection.check();
}
inline void inject(Connection& connection, size_t index, bool down) {
    const bool accepted = index < 256
        ? XTestFakeKeyEvent(connection.get(), index, down, CurrentTime)
        : XTestFakeButtonEvent(connection.get(), index - 255, down, CurrentTime);
    if (!accepted) throw std::runtime_error("X11 rejected input");
    connection.check(); // Clear persisted releases only after server processing.
}
inline void release(Connection& connection, Releases& releases) {
    for (size_t i = 261; i-- > 0;) if (releases.held(i)) {
        inject(connection, i, false); releases.save(i, false);
    }
}
inline void recover() {
    Connection connection; extension(connection);
    Releases releases(connection.get()); release(connection, releases);
}
class Input {
    Connection& connection_;
    Output output_;
    Releases releases_;
    int wheel_ = 0;
    void state(size_t index, bool down) {
        if (releases_.held(index) == down) return;
        if (down) releases_.save(index, true);
        inject(connection_, index, down);
        if (!down) releases_.save(index, false);
    }
    void move(uint16_t x, uint16_t y) {
        const auto& display = output_.display;
        const int px = display.x + int(std::lround(stream_coordinate(x, display.size.width)));
        const int py = display.y + int(std::lround(stream_coordinate(y, display.size.height)));
        // Address the destination root explicitly. XTEST motion can leave the
        // pointer on the previous X screen even when a screen index is supplied.
        XWarpPointer(connection_.get(), None, output_.root, 0, 0, 0, 0, px, py);
        connection_.check();
    }
public:
    Input(Connection& connection, Output output) : connection_(connection), output_(std::move(output)), releases_(connection.get()) {
        extension(connection_); release(connection_, releases_);
    }
    ~Input() { try { release(connection_, releases_); } catch (...) {} }
    bool packet(std::array<uint8_t, 8>& bytes) {
        size_t offset = 0; auto inspected = Clock::now() - std::chrono::seconds(1);
        while (offset < bytes.size()) {
            if (Clock::now() - inspected >= std::chrono::milliseconds(100)) {
                validate(connection_, output_); inspected = Clock::now();
            }
            pollfd pipe{STDIN_FILENO, POLLIN, 0}; const auto ready = poll(&pipe, 1, 20);
            if (ready < 0 && errno == EINTR) continue;
            if (ready < 0) throw std::runtime_error("X11 input pipe poll failed");
            if (!ready) continue;
            const auto count = read(STDIN_FILENO, bytes.data()+offset, bytes.size()-offset);
            if (count < 0 && errno == EINTR) continue;
            if (count < 0) throw std::runtime_error("X11 input pipe failed");
            if (!count) return false;
            offset += size_t(count);
        }
        validate(connection_, output_); return true;
    }
    void apply(const std::array<uint8_t, 8>& bytes) {
        const uint16_t x = bytes[1] | uint16_t(bytes[2]) << 8, y = bytes[3] | uint16_t(bytes[4]) << 8;
        const uint16_t value = bytes[5] | uint16_t(bytes[6]) << 8;
        if (bytes[7] > 1) throw std::runtime_error("Invalid X11 input state");
        switch (bytes[0]) {
        case 1: if (value || bytes[7]) break; move(x,y); return;
        case 2: {
            if (value > 2) break;
            static constexpr unsigned buttons[]{1,3,2};
            move(x,y); state(255+buttons[value], bytes[7]); return;
        }
        case 3:
            if (x || y || bytes[7]) break;
            wheel_ += int16_t(value);
            while (wheel_ >= 120 || wheel_ <= -120) {
                parent_alive();
                const auto button = wheel_ > 0 ? 4 : 5;
                state(255+button, true); state(255+button, false);
                wheel_ += wheel_ > 0 ? -120 : 120;
            }
            return;
        case 4: {
            if (x || y) break;
            // Match the existing Linux physical-key protocol. Xorg's evdev
            // keycodes reserve the first eight values for the core protocol.
            const auto code = evdev_key(value) + 8;
            int minimum, maximum; XDisplayKeycodes(connection_.get(), &minimum, &maximum);
            if (code < unsigned(minimum) || code > unsigned(maximum) || code > 255)
                throw std::runtime_error("Key outside X11 keycode range");
            state(code, bytes[7]); return;
        }
        case 5:
            if (x || y || value || bytes[7]) break;
            release(connection_, releases_); wheel_ = 0; return;
        }
        throw std::runtime_error("Invalid X11 input packet");
    }
};
inline void input(Connection& connection, const Output& output) {
    const auto x = read_value<int32_t>(), y = read_value<int32_t>();
    const auto width = read_value<uint32_t>(), height = read_value<uint32_t>();
    const auto& display = output.display;
    if (x != display.x || y != display.y || width != display.size.logical_width || height != display.size.logical_height)
        throw std::runtime_error("Selected X11 input geometry changed");
    validate(connection, output);
    Input session(connection, output); write_value(magic);
    std::array<uint8_t, 8> packet;
    while (session.packet(packet)) { session.apply(packet); write_value(uint8_t(1)); }
}
inline void run(int argc, const char* const* argv) {
    Connection connection; const auto found = outputs(connection);
    if (argc == 2) {
        std::vector<pab_linux::Display> displays;
        for (const auto& output : found) displays.push_back(output.display);
        inventory(displays); return;
    }
    const auto selected = std::find_if(found.begin(), found.end(), [&](const auto& o) { return o.display.id() == argv[2]; });
    if (selected == found.end()) throw std::runtime_error("Selected X11 display is unavailable");
    if (argc == 3) input(connection, *selected);
    else capture(connection, *selected, number(argv[3],60));
}
}
