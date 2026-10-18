// Isolated acceptance helper; never installed or linked into the product.
// Requires the separately acquired BSD-licensed core/parsec-vdd.h from
// https://github.com/nomi-san/parsec-vdd (retain its license with test artifacts).
// No arguments do nothing. A lease adds exactly one display and removes only
// its returned index. Existing active Parsec displays prohibit a lease.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <chrono>
#include <iostream>
#include <iomanip>
#include <sstream>
#include <string>
#include <thread>
#include <vector>
#include "parsec-vdd.h"

namespace {
struct Output {
    std::wstring name, description;
    DWORD flags;
    DEVMODEW mode{};
    bool has_mode;
};
std::string quoted(const std::wstring& text) {
    std::ostringstream out; out << '"';
    for (wchar_t ch : text) {
        if (ch == L'"' || ch == L'\\') out << '\\' << char(ch);
        else if (ch >= 32 && ch < 127) out << char(ch);
        else out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(ch);
    }
    return out.str() + '"';
}
std::vector<Output> outputs() {
    std::vector<Output> result;
    for (DWORD i = 0;; ++i) {
        DISPLAY_DEVICEW device{}; device.cb = sizeof(device);
        if (!EnumDisplayDevicesW(nullptr, i, &device, 0)) break;
        Output row{device.DeviceName, device.DeviceString, device.StateFlags};
        row.mode.dmSize = sizeof(row.mode);
        row.has_mode = EnumDisplaySettingsExW(device.DeviceName, ENUM_CURRENT_SETTINGS, &row.mode, 0) != 0;
        result.push_back(row);
    }
    return result;
}
void snapshot(const char* phase) {
    std::cout << "{\"phase\":\"" << phase << "\",\"outputs\":[";
    bool first = true;
    for (const auto& row : outputs()) {
        std::cout << (first ? "" : ",") << "{\"name\":" << quoted(row.name)
            << ",\"description\":" << quoted(row.description) << ",\"flags\":" << row.flags
            << ",\"active\":" << ((row.flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP) ? "true" : "false")
            << ",\"mode_known\":" << (row.has_mode ? "true" : "false")
            << ",\"x\":" << row.mode.dmPosition.x << ",\"y\":" << row.mode.dmPosition.y
            << ",\"width\":" << row.mode.dmPelsWidth << ",\"height\":" << row.mode.dmPelsHeight
            << ",\"rotation\":" << row.mode.dmDisplayOrientation << "}";
        first = false;
    }
    std::cout << "]}\n" << std::flush;
}
}

int main(int argc, char** argv) {
    const bool probe = argc == 2 && std::string(argv[1]) == "--probe";
    unsigned duration = 0;
    if (argc == 3 && std::string(argv[1]) == "--lease-seconds") {
        const std::string value(argv[2]);
        if (value.empty() || value.size() > 3 || value.find_first_not_of("0123456789") != std::string::npos) return 2;
        duration = std::stoul(value);
    }
    if (!probe && (duration < 5 || duration > 120)) return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    snapshot("before");
    for (const auto& row : outputs()) {
        if (!probe && row.description.find(L"Parsec") != std::wstring::npos
            && (row.flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP)) {
            std::cerr << "Existing active Parsec display; lease refused\n"; return 3;
        }
    }
    HANDLE driver = parsec_vdd::OpenDeviceHandle(&parsec_vdd::VDD_ADAPTER_GUID);
    if (!driver || driver == INVALID_HANDLE_VALUE) return 4;
    const int version = parsec_vdd::VddVersion(driver);
    std::cout << "{\"driver_minor\":" << version << "}\n" << std::flush;
    if (probe || version < 0) {
        parsec_vdd::CloseDeviceHandle(driver); return version < 0 ? 5 : 0;
    }
    const int index = parsec_vdd::VddAddDisplay(driver);
    if (index < 0 || index >= 16) { parsec_vdd::CloseDeviceHandle(driver); return 6; }
    std::cout << "{\"added_index\":" << index << ",\"lease_seconds\":" << duration << "}\n" << std::flush;
    const auto end = std::chrono::steady_clock::now() + std::chrono::seconds(duration);
    auto next_snapshot = std::chrono::steady_clock::now();
    while (std::chrono::steady_clock::now() < end) {
        parsec_vdd::VddUpdate(driver);
        if (std::chrono::steady_clock::now() >= next_snapshot) {
            snapshot("leased");
            next_snapshot += std::chrono::seconds(2);
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    parsec_vdd::VddRemoveDisplay(driver, index);
    parsec_vdd::CloseDeviceHandle(driver);
    std::cout << "{\"remove_requested_index\":" << index << "}\n" << std::flush;
    std::this_thread::sleep_for(std::chrono::seconds(3));
    snapshot("after");
    return 0; // Caller must compare actual before/after topology, not this exit code.
}
