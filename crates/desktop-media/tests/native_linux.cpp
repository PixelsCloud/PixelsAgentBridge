// Standalone Linux boundary tests; no compositor, capture or user input needed.
#include "../native/linux/capture.h"
#include "../native/linux/input.h"
#include "../native/linux/presenter.h"
#include <cassert>
#include <limits>

template<class F> void rejects(F f) {
    bool rejected = false;
    try { f(); } catch (const std::runtime_error&) { rejected = true; }
    assert(rejected);
}
int main() {
    using namespace pab_linux;
    unsigned checked = 0;
    for (unsigned rotation = 0; rotation < 8; ++rotation) {
        auto g = geometry(3840, 2160, 2, rotation, 1);
        assert(g.width == (rotation & 1 ? 2160u : 3840u));
        assert(g.logical_width == g.width / 2 && g.logical_height == g.height / 2);
        assert(g.scale_milli == 2000); ++checked;
    }
    auto physical = geometry(1920, 1080, 1.5, 0, 2);
    assert(physical.logical_width == 1920 && physical.logical_height == 1080); ++checked;
    rejects([] { geometry(1920, 1080, std::numeric_limits<double>::quiet_NaN(), 0, 1); }); ++checked;
    rejects([] { geometry(1920, 1080, 0, 0, 1); }); ++checked;
    rejects([] { geometry(8192, 8192, 1, 0, 1); }); ++checked;
    rejects([] { geometry(-1, 1080, 1, 0, 1); }); ++checked;
    rejects([] { geometry(1920, 1080, 1, 8, 1); }); ++checked;
    rejects([] { geometry(1920, 1080, 1, 0, 3); }); ++checked;
    uint8_t pixels[64]{};
    spa_chunk chunk{}; chunk.offset = 8; chunk.size = 40; chunk.stride = 20;
    spa_data plane{}; plane.type = SPA_DATA_MemPtr; plane.data = pixels; plane.maxsize = sizeof(pixels); plane.chunk = &chunk;
    spa_buffer frame{}; frame.n_datas = 1; frame.datas = &plane;
    auto view = frame_view(&frame, 4, 2);
    assert(view.data == pixels + 8 && view.stride == 20 && view.size == 40); ++checked;
    plane.type = SPA_DATA_MemFd;
    assert(frame_view(&frame, 4, 2).data == pixels + 8); ++checked;
    plane.type = SPA_DATA_DmaBuf; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    plane.type = SPA_DATA_MemPtr;
    chunk.size = 35; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    chunk.size = 40; chunk.offset = 25; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    chunk.offset = UINT32_MAX; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    chunk.offset = 8; chunk.stride = -1; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    chunk.stride = 20; chunk.flags = SPA_CHUNK_FLAG_CORRUPTED; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    chunk.flags = 0; plane.data = nullptr; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    plane.data = pixels; frame.n_datas = 2; rejects([&] { frame_view(&frame, 4, 2); }); ++checked;
    const uint8_t idr3[]{0,0,1,0x65}, idr4[]{0,0,0,1,0x65}, partial[]{0,0,0,1}, delta[]{0,0,1,0x41};
    assert(keyframe({idr3, sizeof(idr3)}) && keyframe({idr4, sizeof(idr4)})); ++checked;
    assert(!keyframe({partial, sizeof(partial)}) && !keyframe({delta, sizeof(delta)})); ++checked;
    assert(json_string("a\"b\\c\n") == "\"a\\\"b\\\\c\\u000a\""); ++checked;
    for (unsigned pixels : {1u, 1920u, 3840u, 8192u}) {
        assert(stream_coordinate(0, pixels) == 0);
        assert(stream_coordinate(65535, pixels) == pixels - 1);
        assert(stream_coordinate(32768, pixels) >= 0 && stream_coordinate(32768, pixels) <= pixels - 1);
        ++checked;
    }
    rejects([] { stream_coordinate(1, 0); }); ++checked;
    rejects([] { stream_coordinate(1, 8193); }); ++checked;
    for (unsigned vk = 0x41; vk <= 0x5a; ++vk) { assert(evdev_key(vk) > 0 && evdev_key(vk) <= KEY_MAX); ++checked; }
    assert(evdev_key(0x41) == KEY_A && evdev_key(0x5a) == KEY_Z); ++checked;
    assert(evdev_key(0x30) == KEY_0 && evdev_key(0x39) == KEY_9); ++checked;
    assert(evdev_key(0x70) == KEY_F1 && evdev_key(0x79) == KEY_F10 && evdev_key(0x7a) == KEY_F11 && evdev_key(0x87) == KEY_F24); ++checked;
    assert(evdev_key(0x11) == KEY_LEFTCTRL && evdev_key(0xa2) == KEY_LEFTCTRL && evdev_key(0xa3) == KEY_RIGHTCTRL); ++checked;
    assert(evdev_key(0x10) == KEY_LEFTSHIFT && evdev_key(0xa1) == KEY_RIGHTSHIFT); ++checked;
    assert(evdev_key(0x12) == KEY_LEFTALT && evdev_key(0xa5) == KEY_RIGHTALT); ++checked;
    for (unsigned vk : {0u, 1u, 255u, 65535u}) { rejects([&] { evdev_key(vk); }); ++checked; }
    auto wide = viewport(1920,1080,0,960,600);
    assert(wide.x == 0 && wide.y == 30 && wide.width == 960 && wide.height == 540); ++checked;
    auto tall = viewport(1920,1080,1,960,600);
    assert(tall.x == 311 && tall.y == 0 && tall.width == 338 && tall.height == 600); ++checked;
    auto full = viewport(1920,1080,2,1920,1080);
    assert(full.x == 0 && full.y == 0 && full.width == 1920 && full.height == 1080); ++checked;
    rejects([] { viewport(0,1080,0,960,600); }); ++checked;
    rejects([] { viewport(1920,1080,4,960,600); }); ++checked;
    rejects([] { viewport(1920,1080,0,0,600); }); ++checked;
    for (unsigned rotation=0;rotation<4;++rotation) {
        for (int scale : {1,2,3}) {
            const int w=961*scale,h=601*scale;
            const auto v=viewport(1920,1080,rotation,w,h); const auto top=h-v.y-v.height;
            uint16_t x=1,y=1;
            assert(viewer_position(1920,1080,rotation,w,h,v.x,top,false,x,y) && x==0 && y==0); ++checked;
            assert(viewer_position(1920,1080,rotation,w,h,v.x+v.width-1,top+v.height-1,false,x,y) && x==65535 && y==65535); ++checked;
            assert(!viewer_position(1920,1080,rotation,w,h,v.x-1,top,false,x,y)); ++checked;
            assert(viewer_position(1920,1080,rotation,w,h,-100,-100,true,x,y) && x==0 && y==0); ++checked;
            assert(viewer_position(1920,1080,rotation,w,h,w+100,h+100,true,x,y) && x==65535 && y==65535); ++checked;
        }
    }
    uint16_t x=0,y=0;
    assert(!viewer_position(1920,1080,0,960,600,std::numeric_limits<double>::quiet_NaN(),0,false,x,y)); ++checked;
    for (unsigned vk=8;vk<=0xe2;++vk) {
        if (vk==0x10 || vk==0x11 || vk==0x12) continue;
        unsigned code=0; try { code=evdev_key(vk); } catch (...) { continue; }
        assert(windows_key(code+8)==vk); ++checked;
    }
    assert(windows_key(KEY_KPENTER+8)==0x0d && windows_key(0)==0 && windows_key(KEY_MAX+9)==0); ++checked;
    printf("{\"checks\":%u,\"passed\":true}\n", checked);
}
