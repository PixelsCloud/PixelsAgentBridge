#pragma once
#include <array>
#include <functional>
#include <shlobj.h>
#include <sddl.h>

// Runtime recovery data, not configuration or an input log. Only outstanding
// releases are persisted. A process killed after SendInput cannot strand Ctrl
// or a mouse drag indefinitely when its replacement starts.
class InputReleases {
    HANDLE file_ = INVALID_HANDLE_VALUE;
    std::array<uint8_t, 259> held_{};
    void save(size_t index, bool held) {
        LARGE_INTEGER offset{}; offset.QuadPart = index;
        const uint8_t value = held ? 1 : 0;
        DWORD written = 0;
        if (!SetFilePointerEx(file_, offset, nullptr, FILE_BEGIN) ||
            !WriteFile(file_, &value, 1, &written, nullptr) || written != 1 || !FlushFileBuffers(file_))
            throw std::runtime_error("Cannot persist input release state");
        held_[index] = value;
    }
public:
    explicit InputReleases(const std::wstring& path) {
        // Owner, SYSTEM and administrators only. The parent is the local
        // profile of the helper token; SYSTEM media helpers share that profile.
        PSECURITY_DESCRIPTOR descriptor = nullptr;
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(
            L"D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)", SDDL_REVISION_1, &descriptor, nullptr))
            throw std::runtime_error("Cannot protect input recovery state");
        SECURITY_ATTRIBUTES security{sizeof(security), descriptor, FALSE};
        file_ = CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE, 0, &security,
            OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
        LocalFree(descriptor);
        if (file_ == INVALID_HANDLE_VALUE) throw std::runtime_error("Input recovery state is busy or unavailable");
        try {
            BY_HANDLE_FILE_INFORMATION info{};
            if (!GetFileInformationByHandle(file_, &info) || info.nNumberOfLinks != 1 ||
                (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)))
                throw std::runtime_error("Invalid input recovery file");
            LARGE_INTEGER size{};
            if (!GetFileSizeEx(file_, &size) || (size.QuadPart != 0 && size.QuadPart != held_.size()))
                throw std::runtime_error("Invalid input recovery size");
            DWORD count = 0;
            if (size.QuadPart == 0) {
                if (!WriteFile(file_, held_.data(), static_cast<DWORD>(held_.size()), &count, nullptr) ||
                    count != held_.size() || !FlushFileBuffers(file_))
                    throw std::runtime_error("Cannot initialize input recovery state");
            } else {
                if (!ReadFile(file_, held_.data(), static_cast<DWORD>(held_.size()), &count, nullptr) || count != held_.size())
                    throw std::runtime_error("Cannot read input recovery state");
                for (auto value : held_) if (value > 1) throw std::runtime_error("Invalid input recovery flag");
            }
        } catch (...) { CloseHandle(file_); file_ = INVALID_HANDLE_VALUE; throw; }
    }
    InputReleases(const InputReleases&) = delete;
    InputReleases& operator=(const InputReleases&) = delete;
    ~InputReleases() { if (file_ != INVALID_HANDLE_VALUE) CloseHandle(file_); }
    bool held(size_t index) const { return held_.at(index) != 0; }
    void before_press(size_t index) { if (!held(index)) save(index, true); }
    void after_release(size_t index) { if (held(index)) save(index, false); }
    void recover(const std::function<void(size_t)>& release) {
        for (size_t i = held_.size(); i-- > 0;) if (held_[i]) {
            release(i); // Clear only after the OS accepted the release.
            after_release(i);
        }
    }
};

static std::wstring input_recovery_path() {
    wchar_t profile[MAX_PATH]{};
    if (FAILED(SHGetFolderPathW(nullptr, CSIDL_LOCAL_APPDATA | CSIDL_FLAG_CREATE, nullptr, 0, profile)))
        throw std::runtime_error("Input recovery profile unavailable");
    std::wstring path(profile);
    for (const auto* part : {L"\\PixelsAgentBridge", L"\\remote-input"}) {
        path += part;
        if (!CreateDirectoryW(path.c_str(), nullptr) && GetLastError() != ERROR_ALREADY_EXISTS)
            throw std::runtime_error("Cannot create input recovery directory");
        const auto attributes = GetFileAttributesW(path.c_str());
        if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY) ||
            (attributes & FILE_ATTRIBUTE_REPARSE_POINT)) throw std::runtime_error("Invalid input recovery directory");
    }
    DWORD session = 0;
    if (!ProcessIdToSessionId(GetCurrentProcessId(), &session) || session == 0)
        throw std::runtime_error("Input requires an interactive Windows session");
    return path + L"\\session-" + std::to_wstring(session) + L".releases";
}

// Fault-oriented storage test. Never calls SendInput or reads local key state.
static void input_recovery_check() {
    wchar_t directory[MAX_PATH]{}, path[MAX_PATH]{};
    if (!GetTempPathW(MAX_PATH, directory) || !GetTempFileNameW(directory, L"pab", 0, path))
        throw std::runtime_error("Cannot create recovery test file");
    try {
        {
            InputReleases ledger(path);
            ledger.before_press(VK_LCONTROL);
            ledger.before_press(256);
            bool excluded = false;
            try { InputReleases concurrent(path); } catch (...) { excluded = true; }
            if (!excluded) throw std::runtime_error("Concurrent recovery owner was accepted");
            // Simulate an abrupt worker loss: the flags survive object exit.
        }
        {
            InputReleases ledger(path);
            if (!ledger.held(VK_LCONTROL) || !ledger.held(256)) throw std::runtime_error("Lost release recovery state");
            bool rejected = false;
            try { ledger.recover([](size_t) { throw std::runtime_error("simulated OS rejection"); }); }
            catch (...) { rejected = true; }
            if (!rejected || !ledger.held(256)) throw std::runtime_error("Unconfirmed release was cleared");
            unsigned releases = 0;
            ledger.recover([&](size_t index) {
                if (index != 256 && index != VK_LCONTROL) throw std::runtime_error("Recovered an unowned key");
                ++releases;
            });
            if (releases != 2) throw std::runtime_error("Incomplete recovery");
        }
        {
            InputReleases ledger(path);
            ledger.recover([](size_t) { throw std::runtime_error("Replayed a confirmed release"); });
        }
    } catch (...) { DeleteFileW(path); throw; }
    DeleteFileW(path);
}

static INPUT key_input(unsigned key, bool down) {
    INPUT input{}; input.type = INPUT_KEYBOARD; input.ki.wVk = static_cast<WORD>(key);
    input.ki.dwFlags = down ? 0 : KEYEVENTF_KEYUP;
    switch (key) {
        case VK_RCONTROL: case VK_RMENU: case VK_INSERT: case VK_DELETE:
        case VK_HOME: case VK_END: case VK_PRIOR: case VK_NEXT:
        case VK_LEFT: case VK_RIGHT: case VK_UP: case VK_DOWN:
        case VK_NUMLOCK: case VK_DIVIDE: case VK_SNAPSHOT: case VK_LWIN: case VK_RWIN: case VK_APPS:
            input.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY; break;
    }
    return input;
}
static INPUT button_input(unsigned button, bool down) {
    static const DWORD flags[3][2] = {
        {MOUSEEVENTF_LEFTUP, MOUSEEVENTF_LEFTDOWN},
        {MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_RIGHTDOWN},
        {MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MIDDLEDOWN}};
    if (button >= 3) throw std::runtime_error("Invalid mouse button");
    INPUT input{}; input.type = INPUT_MOUSE; input.mi.dwFlags = flags[button][down ? 1 : 0];
    return input;
}
static void send_inputs(INPUT* inputs, unsigned count) {
    if (SendInput(count, inputs, sizeof(INPUT)) != count)
        throw std::runtime_error("Windows rejected remote input (error " + std::to_string(GetLastError()) + ")");
}

// A fresh thread owns no windows/hooks and may bind to the currently active
// desktop. Needed when releasing a key across Default -> Winlogon transitions.
static void recover_input(InputReleases& releases) {
    std::exception_ptr failure;
    std::thread worker([&] {
        // SetThreadDesktop also limits subsequent operations to this handle's
        // granted rights. READ/WRITEOBJECTS alone does not grant input injection.
        HDESK desktop = OpenInputDesktop(0, FALSE, MAXIMUM_ALLOWED);
        if (!desktop) { failure = std::make_exception_ptr(std::runtime_error("Cannot open desktop for input release")); return; }
        HDESK previous = GetThreadDesktop(GetCurrentThreadId());
        try {
            if (!SetThreadDesktop(desktop)) throw std::runtime_error("Cannot bind input release desktop");
            releases.recover([](size_t index) {
                auto input = index < 256 ? key_input(static_cast<unsigned>(index), false) : button_input(static_cast<unsigned>(index - 256), false);
                send_inputs(&input, 1);
            });
        } catch (...) { failure = std::current_exception(); }
        SetThreadDesktop(previous);
        CloseDesktop(desktop);
    });
    worker.join();
    if (failure) std::rethrow_exception(failure);
}

static std::wstring desktop_name(HDESK desktop) {
    wchar_t name[256]{}; DWORD size = 0;
    if (!desktop || !GetUserObjectInformationW(desktop, UOI_NAME, name, sizeof(name), &size))
        throw std::runtime_error("Cannot identify input desktop");
    return name;
}

// Wire layout is explicitly eight bytes; never serialize native struct padding.
// kind (1 move, 2 button, 3 wheel, 4 key, 5 release), x, y, value, down.
static void input_worker(const Display& selected) {
    InputReleases releases(input_recovery_path());
    recover_input(releases);
    const auto initial_desktop = desktop_name(GetThreadDesktop(GetCurrentThreadId()));
    const auto rect = selected.screen.DesktopCoordinates;
    const int32_t expected_x = read_value<int32_t>(), expected_y = read_value<int32_t>();
    const uint32_t expected_width = read_value<uint32_t>(), expected_height = read_value<uint32_t>();
    if (rect.left != expected_x || rect.top != expected_y ||
        rect.right - rect.left != expected_width || rect.bottom - rect.top != expected_height)
        throw std::runtime_error("Selected screen layout changed");
    write_value(magic);
    try {
        for (;;) {
            std::array<uint8_t, 8> bytes{}; read_bytes(bytes.data(), static_cast<uint32_t>(bytes.size()));
            const auto word = [&](size_t offset) { return unsigned(bytes[offset]) | (unsigned(bytes[offset+1]) << 8); };
            const auto kind = bytes[0], down = bytes[7];
            const unsigned x = word(1), y = word(3), value = word(5);
            if (kind < 1 || kind > 5 || down > 1) throw std::runtime_error("Invalid native input packet");
            if (kind == 5) { recover_input(releases); write_value(uint8_t{1}); continue; }
            HDESK desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
            if (!desktop) throw std::runtime_error("Active input desktop unavailable");
            std::wstring active;
            try { active = desktop_name(desktop); } catch (...) { CloseDesktop(desktop); throw; }
            CloseDesktop(desktop);
            if (active != initial_desktop) throw std::runtime_error("Input desktop changed; renew capture generation");
            bool same = false;
            for (const auto& current : displays()) if (screen_id(current) == screen_id(selected)) {
                const auto now = current.screen.DesktopCoordinates;
                same = EqualRect(&rect, &now) && current.screen.Rotation == selected.screen.Rotation;
            }
            if (!same) throw std::runtime_error("Input screen changed; renew capture generation");
            INPUT inputs[2]{}; unsigned count = 0;
            if (kind == 1 || kind == 2) {
                const int vx = GetSystemMetrics(SM_XVIRTUALSCREEN), vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                const int vw = GetSystemMetrics(SM_CXVIRTUALSCREEN), vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                if (vw <= 0 || vh <= 0) throw std::runtime_error("Virtual desktop unavailable");
                const int64_t px = rect.left + int64_t(x) * (rect.right-rect.left-1) / 65535;
                const int64_t py = rect.top + int64_t(y) * (rect.bottom-rect.top-1) / 65535;
                auto& move = inputs[count++]; move.type = INPUT_MOUSE;
                // Pixel centers, rather than edges, preserve the last pixel
                // of a secondary monitor (also with negative desktop origins).
                move.mi.dx = static_cast<LONG>(((px-vx)*65536 + 32768) / vw);
                move.mi.dy = static_cast<LONG>(((py-vy)*65536 + 32768) / vh);
                move.mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            }
            size_t held_index = 259;
            if (kind == 2) { inputs[count++] = button_input(value, down != 0); held_index = 256 + value; }
            if (kind == 3) {
                auto& wheel = inputs[count++]; wheel.type = INPUT_MOUSE;
                wheel.mi.dwFlags = MOUSEEVENTF_WHEEL;
                wheel.mi.mouseData = static_cast<DWORD>(static_cast<int16_t>(value));
            }
            if (kind == 4) {
                if (!value || value > 254) throw std::runtime_error("Invalid virtual key");
                inputs[count++] = key_input(value, down != 0); held_index = value;
            }
            if (held_index < 259 && down) releases.before_press(held_index);
            // Do not release keys/buttons this viewer never held.
            if (held_index < 259 && !down && !releases.held(held_index)) { write_value(uint8_t{1}); continue; }
            send_inputs(inputs, count);
            if (held_index < 259 && !down) releases.after_release(held_index);
            write_value(uint8_t{1});
        }
    } catch (...) {
        try { recover_input(releases); } catch (...) { /* Persisted flags remain for replacement recovery. */ }
        throw;
    }
}
