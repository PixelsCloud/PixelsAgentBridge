#pragma once
// Generated cursor bitmaps only: no desktop capture, windows or input injection.
static void local_cursor_check() {
    unsigned checks = 0;
    const auto require = [&](bool value) { ++checks; if (!value) throw std::runtime_error("Local cursor check failed at " + std::to_string(checks)); };
    const auto packet = [](uint8_t format, const std::vector<uint8_t>& pixels, int64_t x=4, int64_t y=5) {
        std::vector<uint8_t> out;
        const auto put = [&](auto value) { const auto* p=reinterpret_cast<const uint8_t*>(&value); out.insert(out.end(),p,p+sizeof(value)); };
        put(uint8_t(1)); put(uint8_t(1)); put(x); put(y); put(uint32_t(1920)); put(uint32_t(1080)); put(format);
        if (format) { put(uint32_t(4)); put(uint32_t(2)); put(uint32_t(1)); put(uint32_t(1)); out.insert(out.end(),pixels.begin(),pixels.end()); }
        return out;
    };
    pab_cursor::Image image;
    const std::vector<uint8_t> color{0,0,128,128, 0,255,0,255, 0,0,0,0, 255,0,0,255,
        0,0,0,255, 255,255,255,255, 0,128,0,128, 0,0,0,0};
    auto first=packet(1,color); require(image.update(first.data(),first.size()));
    require(image.width==4 && image.height==2 && image.hotspot_x==1 && image.hotspot_y==1);
    auto rgba=image.rgba(); require(rgba[0]==255 && rgba[1]==0 && rgba[2]==0 && rgba[3]==128);
    LocalCursor local; local.replace(image);
    auto roundtrip=win32_cursor_image(local.get());
    require(roundtrip.width==4 && roundtrip.height==2 && roundtrip.hotspot_x==1 && roundtrip.hotspot_y==1);
    if (roundtrip.format!=1 || roundtrip.pixels!=color) {
        std::cerr << "generated cursor roundtrip format=" << unsigned(roundtrip.format) << " bytes=";
        for (auto byte : roundtrip.pixels) std::cerr << unsigned(byte) << ',';
        std::cerr << '\n';
    }
    require(roundtrip.format==1 && roundtrip.pixels==color);
    const auto handle=local.get();
    // Remote motion/hiding cannot warp, hide, replace or redraw our pointer.
    for (int64_t position : {INT64_C(-2147483648),INT64_C(0),INT64_C(2147483647)}) {
        auto move=packet(0,{},position,-position);
        require(!image.update(move.data(),move.size())); require(local.get()==handle && image.pixels==color);
        move[1]=0; require(!image.update(move.data(),move.size()));
    }
    std::vector<uint8_t> masked;
    for (unsigned i=0;i<8;++i) {
        const uint8_t a=i%4>=2 ? 255 : 0,b=i%2 ? 255 : 0;
        masked.insert(masked.end(),{a,a,a,0,b,b,b,0});
    }
    auto xor_packet=packet(2,masked); require(image.update(xor_packet.data(),xor_packet.size()));
    local.replace(image); roundtrip=win32_cursor_image(local.get());
    require(roundtrip.format==2 && roundtrip.pixels==masked);
    for (size_t cut : {size_t(0),size_t(1),size_t(26),size_t(42),first.size()-1}) {
        bool rejected=false; try { image.update(first.data(),cut); } catch (const std::runtime_error&) { rejected=true; }
        require(rejected);
    }
    auto invalid=first; invalid[27]=1; invalid[28]=2; // width 513
    bool rejected=false; try { image.update(invalid.data(),invalid.size()); } catch (const std::runtime_error&) { rejected=true; }
    require(rejected);
    const uint8_t clear=0; require(image.update(&clear,1)); local.replace(image);
    require(image.pixels.empty() && local.get()==LoadCursorW(nullptr,IDC_ARROW));
    std::cout << "{\"status\":\"passed\",\"local_cursor_checks\":" << checks << ",\"desktop_access\":false}\n";
}
