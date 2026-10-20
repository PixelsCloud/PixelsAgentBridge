#include "../native/linux/uinput.h"
#include <cassert>
#include <cstdio>
using namespace pab_linux::virtual_input;
int main() {
    Geometry left;
    left.left = -1920; left.top = -200; left.width = 4480; left.height = 1640;
    left.x = -1920; left.y = 0; left.screen_width = 1920; left.screen_height = 1080;
    validate(left);
    assert(coordinate(0, left.x, left.screen_width) == -1920);
    assert(coordinate(65535, left.x, left.screen_width) == -1);
    assert(coordinate(32768, left.x, left.screen_width) == -960);
    Geometry right = left; right.x = 0; right.y = -200; right.screen_width = 2560; right.screen_height = 1440;
    validate(right);
    assert(coordinate(65535, right.x, right.screen_width) == 2559);
    assert(coordinate(0, right.y, right.screen_height) == -200);
    assert(coordinate(65535, right.y, right.screen_height) == 1239);
    assert(coordinate(65535, -50, 1) == -50);
    auto rejects = [](const Geometry& value) {
        try { validate(value); } catch (const std::runtime_error&) { return; }
        assert(false);
    };
    auto invalid = left; invalid.width = 0; rejects(invalid);
    invalid = left; invalid.x = -1921; rejects(invalid);
    invalid = right; invalid.screen_width = 2561; rejects(invalid);
    invalid = left; invalid.left = INT32_MAX-1; rejects(invalid);
    invalid = left; invalid.width = 262145; rejects(invalid);
    invalid = left; invalid.version = 2; rejects(invalid);
    puts("uinput geometry: negative origins, multi-screen extents and invalid bounds passed");
}
