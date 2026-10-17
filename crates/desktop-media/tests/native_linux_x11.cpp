// Run only on an isolated X server (verify_linux_x11.py owns it).
#include "../native/linux/x11_capture.h"
#include <cassert>

template<class F> void rejects(F f) {
    bool rejected = false;
    try { f(); } catch (const std::runtime_error&) { rejected = true; }
    assert(rejected);
}
int main() {
    using namespace pab_linux::x11;
    unsigned checks = 0;
    setenv("XDG_SESSION_TYPE", "wayland", 1); assert(!selected()); ++checks;
    setenv("XDG_SESSION_TYPE", "tty", 1); assert(!selected()); ++checks;
    unsetenv("XDG_SESSION_TYPE"); setenv("WAYLAND_DISPLAY", "wayland-0", 1);
    assert(!selected()); ++checks;
    unsetenv("WAYLAND_DISPLAY"); assert(selected()); ++checks;
    setenv("XDG_SESSION_TYPE", "x11", 1); assert(selected()); ++checks;
    XImage image{};
    image.width = 4; image.height = 2; image.format = ZPixmap; image.bits_per_pixel = 32;
    image.byte_order = LSBFirst; image.red_mask = 0xff0000; image.green_mask = 0xff00;
    image.blue_mask = 0xff; image.bytes_per_line = 20;
    assert(image_size(image, 4, 2) == 40); ++checks;
    for (auto member : {&XImage::width, &XImage::height, &XImage::bits_per_pixel,
                        &XImage::bytes_per_line}) {
        const auto old = image.*member; image.*member = -1;
        rejects([&] { image_size(image, 4, 2); }); image.*member = old; ++checks;
    }
    image.byte_order = MSBFirst; rejects([&] { image_size(image, 4, 2); }); ++checks;
    image.byte_order = LSBFirst; image.red_mask = 0xff;
    rejects([&] { image_size(image, 4, 2); }); image.red_mask = 0xff0000; ++checks;
    unsigned char data[40]{}; image.data = reinterpret_cast<char*>(data);
    Output output{{"0:test", "test", 100, 200, {4,2,4,2,1000}, true, 1, "x11"}, 0, 1};
    unsigned long argb[]{0xffff0000, 0xff00ff00, 0xff0000ff, 0x80800000};
    XFixesCursorImage cursor{};
    cursor.width = cursor.height = 2; cursor.x = 100; cursor.y = 200;
    cursor.xhot = cursor.yhot = 1; cursor.pixels = argb;
    blend_cursor(image, output, cursor);
    assert(data[2] == 128 && data[0] == 0 && data[1] == 0); ++checks;
    for (size_t i = 4; i < sizeof(data); ++i) assert(data[i] == 0);
    cursor.width = 513; rejects([&] { blend_cursor(image, output, cursor); }); ++checks;
    cursor.width = 2; cursor.pixels = nullptr; rejects([&] { blend_cursor(image, output, cursor); }); ++checks;
    pab_linux::x11::Connection connection;
    const auto screens = outputs(connection);
    assert(screens.size() == 2); ++checks;
    assert(screens[0].display.id() != screens[1].display.id()); ++checks;
    for (const auto& screen : screens) {
        validate(connection, screen); ++checks;
        auto stale = screen; stale.display.layout_serial++;
        rejects([&] { validate(connection, stale); }); ++checks;
        stale = screen; stale.display.x++;
        rejects([&] { validate(connection, stale); }); ++checks;
        const auto pixel = screen.screen == 0 ? 0xff0000 : 0x00ff00;
        XSetWindowBackground(connection.get(), screen.root, pixel);
        XClearWindow(connection.get(), screen.root); connection.check();
        for (unsigned i = 0; i < 20; ++i) {
            Image frame(connection, screen);
            auto& actual = frame.read(screen);
            const auto value = XGetPixel(&actual, 10, 10);
            assert(value == static_cast<unsigned long>(pixel)); ++checks;
        }
    }
    printf("{\"checks\":%u,\"x_screens\":%zu,\"shared_images_reclaimed\":40}\n", checks, screens.size());
}
