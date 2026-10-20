#pragma once
#include "../software_codec_ffi.h"
#include "presenter.h"

namespace pab_linux {
struct DecoderDrop { void operator()(void* p) const { pab_soft_decoder_drop(p); } };
inline void render_read(Presenter& view, void* output, size_t size) {
    auto* bytes = static_cast<uint8_t*>(output);
    while (size) {
        view.poll_events(); pollfd pipe{STDIN_FILENO,POLLIN,0};
        auto ready = poll(&pipe,1,10);
        if (ready < 0 && errno == EINTR) continue;
        if (ready < 0) throw std::runtime_error("Native render pipe failed");
        if (!ready) continue;
        auto count = read(STDIN_FILENO,bytes,size);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("Native render parent ended");
        bytes += count; size -= count;
    }
}
template<class T> T render_read(Presenter& view) { T result{}; render_read(view,&result,sizeof(result)); return result; }
inline void render(unsigned width, unsigned height, unsigned fps, std::string mode, unsigned rotation) {
    const std::string fullscreen_suffix = "-fullscreen";
    const bool fullscreen = mode.size() > fullscreen_suffix.size() && mode.compare(mode.size()-fullscreen_suffix.size(),fullscreen_suffix.size(),fullscreen_suffix) == 0;
    if (fullscreen) mode.resize(mode.size()-fullscreen_suffix.size());
    const std::string redraw_suffix = "-redraw";
    const bool redraw = mode.size() > redraw_suffix.size() && mode.compare(mode.size()-redraw_suffix.size(),redraw_suffix.size(),redraw_suffix) == 0;
    if (redraw) mode.resize(mode.size()-redraw_suffix.size());
    const std::string lifecycle_suffix = "-lifecycle";
    const bool lifecycle = mode.size() > lifecycle_suffix.size() && mode.compare(mode.size()-lifecycle_suffix.size(),lifecycle_suffix.size(),lifecycle_suffix) == 0;
    if (lifecycle) mode.resize(mode.size()-lifecycle_suffix.size());
    if ((fullscreen && (lifecycle || redraw)) || (lifecycle && redraw)) throw std::runtime_error("Choose one window diagnostic");
    if (!fps || fps > 60 || (mode != "stream" && mode != "stream-software" && mode != "stream-verify" && mode != "stream-verify-software"))
        throw std::runtime_error("Unsupported Linux render mode");
    Presenter view(width,height,rotation,mode == "stream-verify" || mode == "stream-verify-software",fullscreen);
    std::unique_ptr<void,DecoderDrop> decoder(pab_soft_decoder_new(width,height));
    if (!decoder) throw std::runtime_error("Cannot create bundled software decoder");
    view.poll_events();
    fprintf(stderr,"media decoder=openh264_software renderer=%s pixel_format=nv12 shader_conversion=true\n",view.renderer());
    write_value(magic); write_value(uint32_t(2));
    write_value(uint8_t(4)); write_value(uint8_t(2));
    uint64_t next = 0;
    for (;;) {
        const auto size = render_read<uint32_t>(view);
        if (!size) break;
        if (size > max_packet) throw std::runtime_error("Native render packet exceeds limit");
        const auto index = render_read<uint64_t>(view);
        if (index != next) throw std::runtime_error("Native render packet reordered");
        std::vector<uint8_t> bytes(size); render_read(view,bytes.data(),bytes.size());
        PabCodecBytes output{};
        if (!pab_soft_decode(decoder.get(),bytes.data(),bytes.size(),&output) || !output.size)
            throw std::runtime_error("Native software decoder has no valid output");
        write_value(uint8_t(1)); write_value(index);
        // The codec's borrowed NV12 storage remains live through upload. No
        // intermediate RGB conversion or additional CPU frame copy is needed.
        view.present(output,index);
        write_value(uint8_t(2)); write_value(index); ++next;
    }
    auto drawable = view.drawable_size();
    fprintf(stderr,"media drawable_width=%d drawable_height=%d\n",drawable.first,drawable.second);
    if (lifecycle || redraw) view.lifecycle_check(lifecycle);
    write_value(uint8_t(3)); write_value(view.presented()); write_value(view.checks()); write_value(uint32_t(view.max_error()));
}
}
