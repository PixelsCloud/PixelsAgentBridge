#pragma once
#include <cstddef>
#include <cstdint>
#include <deque>
#include <utility>

// DXGI presentation observations are telemetry, not the GPU submission queue.
// A missing observation must neither block the next frame nor count as display.
class PresentationHistory {
    std::deque<std::pair<uint32_t, bool>> pending_;
public:
    static constexpr std::size_t capacity = 3;
    void submitted(uint32_t id, bool video) {
        if (pending_.size() == capacity) pending_.pop_front();
        pending_.emplace_back(id, video);
    }
    uint64_t observed(uint32_t id) {
        uint64_t displayed = 0;
        // Present IDs wrap at UINT32_MAX. The retained history spans only a
        // few submissions; older observations cannot acknowledge newer IDs.
        while (!pending_.empty() && uint32_t(id - pending_.front().first) <= UINT32_MAX / 2) {
            const auto present = pending_.front();
            pending_.pop_front();
            if (present.first == id && present.second) ++displayed;
        }
        return displayed;
    }
    bool empty() const { return pending_.empty(); }
    std::size_t size() const { return pending_.size(); }
    void clear() { pending_.clear(); }
};
