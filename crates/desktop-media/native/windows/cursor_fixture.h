#pragma once
// Generated shapes only. This check does not inspect or move the OS cursor.
static void cursor_shape_check() {
    unsigned checks = 0;
    const auto require = [&](bool condition) {
        ++checks;
        if (!condition) throw std::runtime_error("Cursor shape check failed");
    };
    DXGI_OUTDUPL_POINTER_SHAPE_INFO shape{};
    shape.Type = DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME;
    shape.Width = 9; shape.Height = 4; shape.Pitch = 4; shape.HotSpot = {8,1};
    std::vector<uint8_t> raw(16, 0);
    raw[0] = 0x30; raw[8] = 0x50; raw[5] = 0x80; raw[13] = 0x80;
    auto image = cursor_image(shape, raw);
    require(image.format == 2 && image.width == 9 && image.height == 2 && image.hotspot_x == 8 && image.hotspot_y == 1);
    for (uint8_t background : {uint8_t(0),uint8_t(0x55),uint8_t(0xff)}) {
        for (unsigned x = 0; x < 4; ++x) for (unsigned c = 0; c < 3; ++c) {
            const uint8_t actual = (background & image.pixels[x*8+c]) ^ image.pixels[x*8+4+c];
            const uint8_t expected = x == 0 ? 0 : x == 1 ? 255 : x == 2 ? background : uint8_t(~background);
            require(actual == expected);
        }
    }
    require(image.pixels[(9+8)*8] == 255 && image.pixels[(9+8)*8+4] == 255);
    shape.Type = DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR;
    shape.Width = 2; shape.Height = 1; shape.Pitch = 12; shape.HotSpot = {1,0};
    raw = {10,20,30,0, 40,50,60,255, 99,99,99,99};
    image = cursor_image(shape, raw);
    require(image.pixels == std::vector<uint8_t>({0,0,0,0,10,20,30,0, 255,255,255,0,40,50,60,0}));
    shape.Type = DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR;
    raw = {100,200,255,128, 40,50,60,0, 99,99,99,99};
    image = cursor_image(shape, raw);
    require(image.format == 1 && image.pixels == std::vector<uint8_t>({50,100,128,128,0,0,0,0}));
    require(cursor_coordinate(-1920, 20, 1) == -1899);
    require(cursor_coordinate(0, -10, 2) == -8);
    for (unsigned failure = 0; failure < 9; ++failure) {
        auto invalid = shape; auto bytes = raw;
        switch (failure) {
        case 0: invalid.Width = 513; break;
        case 1: invalid.Height = 0; break;
        case 2: invalid.Pitch = 4; break;
        case 3: invalid.Pitch = UINT32_MAX; break;
        case 4: invalid.HotSpot.x = 2; break;
        case 5: invalid.HotSpot.y = -1; break;
        case 6: bytes.resize(2); break;
        case 7: invalid.Type = DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME; invalid.Height = 3; break;
        case 8: invalid.Type = DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR; break;
        }
        bool rejected = false;
        try { cursor_image(invalid, bytes); } catch (const std::runtime_error&) { rejected = true; }
        require(rejected);
    }
    bool overflow = false;
    try { cursor_coordinate(INT32_MAX, 1, 1); } catch (const std::runtime_error&) { overflow = true; }
    require(overflow);
    std::cout << "cursor shape checks=" << checks << '\n';
}
