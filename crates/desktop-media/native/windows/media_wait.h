#pragma once
#include <windows.h>
#include <chrono>
#include <thread>

// Sleep's default timer resolution can turn a 2 ms IPC poll into a 15 ms
// delay. Use a timer owned by this worker thread, without changing the system
// timer resolution or spinning while the parent is idle.
namespace media_wait {
class Timer {
    HANDLE handle_ = nullptr;
    bool high_resolution_ = false;
public:
    explicit Timer(bool legacy = false) {
        if (!legacy) {
            // Available since Windows 10 1803; keep the ordinary timer fallback
            // for older systems and environments which reject this flag.
            handle_ = CreateWaitableTimerExW(nullptr, nullptr, 0x00000002,
                TIMER_MODIFY_STATE | SYNCHRONIZE);
            high_resolution_ = handle_ != nullptr;
        }
        if (!handle_) handle_ = CreateWaitableTimerW(nullptr, FALSE, nullptr);
    }
    ~Timer() { if (handle_) CloseHandle(handle_); }
    Timer(const Timer&) = delete;
    Timer& operator=(const Timer&) = delete;
    bool high_resolution() const { return high_resolution_; }

    void until(std::chrono::steady_clock::time_point deadline) {
        for (;;) {
            const auto remaining = deadline - std::chrono::steady_clock::now();
            if (remaining <= decltype(remaining)::zero()) return;
            if (!handle_) { std::this_thread::sleep_for(remaining); continue; }
            // Relative due times are negative, in 100 ns units. Round up so a
            // sub-tick request cannot accidentally become an absolute time.
            const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(remaining).count();
            LARGE_INTEGER due{}; due.QuadPart = -(ns / 100 + (ns % 100 != 0));
            if (!SetWaitableTimer(handle_, &due, 0, nullptr, nullptr, FALSE) ||
                WaitForSingleObject(handle_, INFINITE) != WAIT_OBJECT_0) {
                std::this_thread::sleep_for(remaining);
            }
        }
    }
};

inline Timer& current() { static thread_local Timer timer; return timer; }
inline void until(std::chrono::steady_clock::time_point deadline) { current().until(deadline); }
template<class Rep, class Period> inline void sleep(std::chrono::duration<Rep, Period> duration) {
    until(std::chrono::steady_clock::now() + duration);
}
}
