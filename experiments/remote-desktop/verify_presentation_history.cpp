#include "../../crates/desktop-media/native/windows/presentation_history.h"
#include <cassert>
#include <iostream>

int main() {
    PresentationHistory history;
    history.submitted(1, true);
    history.submitted(2, true);
    history.submitted(3, true);
    assert(history.observed(2) == 1); // ID 1 was skipped, not displayed.
    assert(history.observed(2) == 0); // Repeated observations grant no credit.
    assert(history.observed(3) == 1 && history.empty());

    // Simulate unavailable/disjoint/stale statistics for a long interval.
    for (uint32_t id = 4; id <= 100000; ++id) {
        history.submitted(id, true);
        assert(history.size() <= PresentationHistory::capacity);
        assert(history.observed(3) == 0);
    }
    assert(history.observed(99997) == 0); // Evicted IDs stay unconfirmed.
    assert(history.observed(100000) == 1 && history.empty());
    history.submitted(100001, false); // Cursor/static repaint is not video.
    assert(history.observed(100001) == 0 && history.empty());

    history.submitted(UINT32_MAX - 1, true);
    history.submitted(UINT32_MAX, true);
    history.submitted(0, true);
    assert(history.observed(UINT32_MAX) == 1);
    assert(history.observed(UINT32_MAX) == 0);
    assert(history.observed(0) == 1 && history.empty());
    history.submitted(1, true);
    history.clear();
    assert(history.observed(1) == 0 && history.empty());
    std::cout << "presentation history: missing/stale/skipped/repaint/wrapped observations passed\n";
}
