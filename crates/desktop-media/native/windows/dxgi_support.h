#pragma once
// Shared DXGI inventory and texture lease helpers; no input injection.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_2.h>
#include <shellscalingapi.h>
#include <wrl/client.h>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;
using Clock = std::chrono::steady_clock;

inline std::string quoted(const wchar_t* value) {
    int n = WideCharToMultiByte(CP_UTF8, 0, value, -1, nullptr, 0, nullptr, nullptr);
    std::string utf8(n, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value, -1, utf8.data(), n, nullptr, nullptr);
    std::ostringstream out;
    out << '"';
    for (int i = 0; i < n - 1; ++i) {
        unsigned char c = utf8[i];
        if (c == '"' || c == '\\') out << '\\' << c;
        else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c);
        else out << c;
    }
    out << '"';
    return out.str();
}

inline void check(HRESULT hr, const char* stage) {
    if (FAILED(hr)) {
        std::ostringstream message;
        message << stage << " HRESULT=0x" << std::hex << uint32_t(hr);
        throw std::runtime_error(message.str());
    }
}

struct Display {
    ComPtr<IDXGIAdapter1> adapter;
    ComPtr<IDXGIOutput1> output;
    DXGI_ADAPTER_DESC1 gpu{};
    DXGI_OUTPUT_DESC screen{};
    UINT dpi_x = 96, dpi_y = 96;
};

inline bool same_display_layout(const Display& selected, const Display& current) {
    return EqualRect(&selected.screen.DesktopCoordinates, &current.screen.DesktopCoordinates) &&
        selected.screen.Rotation == current.screen.Rotation &&
        selected.screen.Monitor == current.screen.Monitor &&
        selected.dpi_x == current.dpi_x && selected.dpi_y == current.dpi_y;
}

inline void validate_selected_display_layout(const Display& selected) {
    Display current;
    check(selected.output->GetDesc(&current.screen), "Capture output description");
    if (!current.screen.AttachedToDesktop)
        throw std::runtime_error("Capture output detached; refresh generation");
    check(GetDpiForMonitor(current.screen.Monitor, MDT_EFFECTIVE_DPI, &current.dpi_x, &current.dpi_y), "Capture monitor DPI");
    if (!same_display_layout(selected, current))
        throw std::runtime_error("Screen changed; refresh generation");
}

inline std::vector<Display> displays() {
    ComPtr<IDXGIFactory1> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    std::vector<Display> found;
    for (UINT a = 0;; ++a) {
        ComPtr<IDXGIAdapter1> adapter;
        HRESULT hr = factory->EnumAdapters1(a, &adapter);
        if (hr == DXGI_ERROR_NOT_FOUND) break;
        check(hr, "EnumAdapters1");
        for (UINT o = 0;; ++o) {
            ComPtr<IDXGIOutput> output;
            hr = adapter->EnumOutputs(o, &output);
            if (hr == DXGI_ERROR_NOT_FOUND) break;
            check(hr, "EnumOutputs");
            Display d;
            d.adapter = adapter;
            check(output.As(&d.output), "IDXGIOutput1");
            check(adapter->GetDesc1(&d.gpu), "Adapter.GetDesc1");
            check(output->GetDesc(&d.screen), "Output.GetDesc");
            if (d.screen.AttachedToDesktop) {
                check(GetDpiForMonitor(d.screen.Monitor, MDT_EFFECTIVE_DPI, &d.dpi_x, &d.dpi_y), "Monitor DPI");
                found.push_back(std::move(d));
            }
        }
    }
    return found;
}

inline void identify(const Display& d, size_t index) {
    const auto& r = d.screen.DesktopCoordinates;
    std::cout << "{\"index\":" << index << ",\"device_name\":" << quoted(d.screen.DeviceName)
        << ",\"adapter\":" << quoted(d.gpu.Description)
        << ",\"adapter_luid\":\"" << d.gpu.AdapterLuid.HighPart << ':' << d.gpu.AdapterLuid.LowPart
        << "\",\"x\":" << r.left << ",\"y\":" << r.top
        << ",\"width\":" << r.right - r.left << ",\"height\":" << r.bottom - r.top
        << ",\"dxgi_rotation\":" << d.screen.Rotation << '}';
}

// DXGI frame is released even if texture inspection fails.
struct FrameLease {
    IDXGIOutputDuplication* duplication;
    ~FrameLease() { if (duplication) duplication->ReleaseFrame(); }
};
