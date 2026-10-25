#pragma once
#include "media_wait.h"
#include <algorithm>
#include <iostream>
#include <stdexcept>
#include <vector>

static void media_timing_check() {
    using namespace std::chrono;
    DWORD before = 0, after = 0;
    if (!GetProcessHandleCount(GetCurrentProcess(), &before))
        throw std::runtime_error("Timer handle count unavailable");
    bool high_resolution = false;
    std::vector<double> samples;
    {
        media_wait::Timer timer;
        high_resolution = timer.high_resolution();
        for (unsigned i = 0; i < 24; ++i) {
            const auto start = steady_clock::now();
            timer.until(start + milliseconds(2));
            const auto elapsed = steady_clock::now() - start;
            if (elapsed < milliseconds(2)) throw std::runtime_error("Media timer woke early");
            samples.push_back(duration<double, std::milli>(elapsed).count());
        }
        timer.until(steady_clock::now() - seconds(1));
        // Exercise the supported fallback independently of host capabilities.
        media_wait::Timer fallback(true);
        const auto start = steady_clock::now();
        fallback.until(start + milliseconds(2));
        if (steady_clock::now() - start < milliseconds(2))
            throw std::runtime_error("Fallback timer woke early");
    }
    if (!GetProcessHandleCount(GetCurrentProcess(), &after) || after != before)
        throw std::runtime_error("Media timer leaked a handle");
    std::sort(samples.begin(), samples.end());
    std::cout << "{\"high_resolution\":" << (high_resolution ? "true" : "false")
        << ",\"samples\":" << samples.size() << ",\"requested_ms\":2,\"median_ms\":"
        << samples[samples.size()/2] << ",\"max_ms\":" << samples.back()
        << ",\"fallback_checked\":true,\"handles_released\":true}\n";
}
