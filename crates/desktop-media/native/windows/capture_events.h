#pragma once
#include "dxgi_support.h"

// Two per-capture, session-local kernel events. No persistent files or network
// protocol changes. Both workers use their existing helper identity and DACL.
class CaptureEvents {
    HANDLE request_ = nullptr, active_ = nullptr;
    static bool signaled(HANDLE event) {
        const auto result = WaitForSingleObject(event, 0);
        if (result != WAIT_OBJECT_0 && result != WAIT_TIMEOUT)
            throw std::runtime_error("Capture event wait failed");
        return result == WAIT_OBJECT_0;
    }
    static HANDLE event(const std::wstring& name, bool owner, DWORD access) {
        HANDLE result = owner ? CreateEventW(nullptr, TRUE, FALSE, name.c_str()) :
            OpenEventW(access, FALSE, name.c_str());
        if (!result) throw std::runtime_error("Capture event unavailable");
        if (owner && GetLastError() == ERROR_ALREADY_EXISTS) {
            CloseHandle(result);
            throw std::runtime_error("Capture event already owned");
        }
        return result;
    }
public:
    CaptureEvents(const std::string& token, bool owner) {
        if (token.size() != 36) throw std::runtime_error("Invalid capture token");
        for (size_t i = 0; i < token.size(); ++i) {
            const bool dash = i == 8 || i == 13 || i == 18 || i == 23;
            if (dash ? token[i] != '-' : !((token[i] >= '0' && token[i] <= '9') || (token[i] >= 'a' && token[i] <= 'f')))
                throw std::runtime_error("Invalid capture token");
        }
        const auto name = L"Local\\PixelsRemoteDesktopCapture-" + std::wstring(token.begin(), token.end());
        request_ = event(name + L"-request", owner, SYNCHRONIZE | EVENT_MODIFY_STATE);
        try { active_ = event(name + L"-active", owner, SYNCHRONIZE); }
        catch (...) { CloseHandle(request_); throw; }
    }
    ~CaptureEvents() { CloseHandle(active_); CloseHandle(request_); }
    CaptureEvents(const CaptureEvents&) = delete;
    CaptureEvents& operator=(const CaptureEvents&) = delete;
    bool requested() const { return signaled(request_); }
    bool active() const { return signaled(active_); }
    void request() { if (!SetEvent(request_)) throw std::runtime_error("Capture request failed"); }
    void activate() { if (!SetEvent(active_)) throw std::runtime_error("Capture activation failed"); }
};
