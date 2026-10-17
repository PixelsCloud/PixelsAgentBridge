// Native macOS worker: compressed frames use the existing private pipe and
// authenticated reliable transport. No application/network configuration here.
#include "encoder.h"
#include "capture.h"
#include "cursor_source.h"
#include "decoder.h"
#include "presenter.h"
#include "host.h"
#include <csignal>

namespace pab_mac {
// Opt-in diagnostic only. The hardware capture path never maps CPU pixels.
static OwnedCF<CVPixelBufferRef> synthetic(unsigned width, unsigned height, uint64_t index) {
    NSDictionary* attributes = @{(__bridge NSString*)kCVPixelBufferIOSurfacePropertiesKey: @{}};
    CVPixelBufferRef image = nullptr;
    check(CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        (__bridge CFDictionaryRef)attributes, &image), "Synthetic image");
    OwnedCF<CVPixelBufferRef> owned(image);
    check(CVPixelBufferLockBaseAddress(image, 0), "Synthetic mapping");
    for (unsigned q = 0; q < 4; ++q) {
        const double r = 24 + (index * 3 + q * 43) % 208;
        const double g = 24 + (index * 7 + q * 67) % 208;
        const double b = 24 + (index * 11 + q * 97) % 208;
        const uint8_t y = std::lround(16 + (0.2126*r + 0.7152*g + 0.0722*b)*219/255);
        const uint8_t u = std::lround(128 + (-0.114572*r - 0.385428*g + 0.5*b)*224/255);
        const uint8_t v = std::lround(128 + (0.5*r - 0.454153*g - 0.045847*b)*224/255);
        const unsigned x0 = (q % 2)*width/2, x1 = (q % 2 + 1)*width/2;
        const unsigned y0 = (q / 2)*height/2, y1 = (q / 2 + 1)*height/2;
        auto* luma = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image, 0));
        auto* chroma = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image, 1));
        for (unsigned row = y0; row < y1; ++row)
            memset(luma + row*CVPixelBufferGetBytesPerRowOfPlane(image, 0) + x0, y, x1 - x0);
        for (unsigned row = y0/2; row < y1/2; ++row)
            for (unsigned column = x0/2; column < x1/2; ++column) {
                auto* pixel = chroma + row*CVPixelBufferGetBytesPerRowOfPlane(image, 1) + column*2;
                pixel[0] = u; pixel[1] = v;
            }
    }
    check(CVPixelBufferUnlockBaseAddress(image, 0), "Synthetic unmapping");
    return owned;
}
static void capture(const Display& display, unsigned fps, const std::string& mode) {
    const bool software = mode == "synthetic-software";
    const bool fail_initial = mode == "synthetic-fail-encoder-init";
    const bool fail_encode = mode == "synthetic-fail-encoder";
    const bool desktop = mode == "desktop" || mode == "desktop-separate-cursor";
    if (!desktop && mode != "synthetic" && !software && !fail_initial && !fail_encode)
        throw std::runtime_error("Unknown capture mode");
    const auto start = Clock::now(); auto next = start;
    VideoEncoder encoder(display.width, display.height, fps, software, fail_initial, fail_encode);
    std::function<OwnedCF<CVPixelBufferRef>(Clock::time_point&)> source;
    if (desktop) {
        if (@available(macOS 12.3, *)) {
            const auto capture = std::make_shared<Capture>(display, fps, mode == "desktop");
            source = [capture](Clock::time_point& time) { return capture->next(time); };
        }
        else throw std::runtime_error("Remote screen capture requires macOS 12.3 or later");
    }
    fprintf(stderr, "media encoder=%s source=%s cpu_pixel_mappings_per_frame=%u\n",
        encoder.software() ? "openh264_software" : "videotoolbox_hardware",
        mode.c_str(), (desktop ? 0u : 1u) + (encoder.software() ? 1u : 0u));
    write_value(magic); write_value(display.width); write_value(display.height);
    uint64_t index = 0;
    for (;;) { @autoreleasepool {
        const auto command = read_value<uint8_t>();
        if (command == 0) return;
        if (command != 1) throw std::runtime_error("Invalid capture command");
        while (Clock::now() < next) { parent_alive(); pump(); }
        auto sampled = Clock::now();
        OwnedCF<CVPixelBufferRef> image;
        if (source) image = source(sampled);
        else image = synthetic(display.width, display.height, index);
        const auto packet = encoder.encode(image.get(), index);
        write_value(static_cast<uint32_t>(packet.bytes.size())); write_value(index);
        write_value(static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(sampled-start).count()));
        write_value(static_cast<uint32_t>(packet.key)); write_bytes(packet.bytes.data(), packet.bytes.size());
        ++index;
        next = std::max(next + std::chrono::microseconds(1000000/fps), Clock::now());
    } }
}
static void render(unsigned width, unsigned height, unsigned fps, unsigned rotation, bool verify, bool software, bool fail_decoder, bool lifecycle) {
    if (uint64_t(width)*height > 33554432) throw std::runtime_error("Render dimensions outside bounds");
    VideoDecoder decoder(width, height, fps, software, fail_decoder);
    NativePresenter presenter(width, height, rotation, verify);
    const auto idle = [&] { presenter.pump_events(); };
    write_value(magic); write_value(uint32_t(2));
    uint64_t expected = 0;
    double decode_us = 0, render_us = 0;
    for (;;) { @autoreleasepool {
        idle();
        const auto size = read_value<uint32_t>(idle);
        if (!size) {
            presenter.finish();
            if (lifecycle) presenter.verify_lifecycle();
            write_value(uint8_t(3)); write_value(presenter.displayed());
            write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            if (verify && expected) fprintf(stderr, "media decode_ms=%.3f render_ms=%.3f drawable_ms=%.3f gpu_wait_ms=%.3f draws=%llu frames=%llu\n",
                decode_us/expected/1000, render_us/expected/1000, presenter.drawable_us/expected/1000,
                presenter.gpu_wait_us/expected/1000, presenter.draw_count, expected);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet outside bounds");
        const auto index = read_value<uint64_t>(idle);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), packet.size(), idle);
        auto before = Clock::now(); auto image = decoder.decode(packet, index);
        decode_us += std::chrono::duration<double,std::micro>(Clock::now()-before).count();
        if (index == 0) {
            write_value(uint8_t(4)); write_value(uint8_t(decoder.software() ? 2 : 3));
            fprintf(stderr, "media decoder=%s cpu_pixel_mappings_per_frame=%u\n",
                decoder.software() ? "openh264_software" : "videotoolbox_hardware", decoder.software() ? 1 : 0);
        }
        write_value(uint8_t(1)); write_value(index);
        before = Clock::now(); presenter.render(image.get(), index);
        render_us += std::chrono::duration<double,std::micro>(Clock::now()-before).count();
        // Submission/command completion can precede, or fail to produce, a
        // positively timestamped presentation. Keep this frame until it is
        // actually shown, including static redraw after occlusion/restore.
        // EOF still cancels a hidden standalone viewer through parent_alive.
        while (presenter.displayed() <= index) { parent_alive(); idle(); pump(); }
        if (presenter.displayed() != index + 1) throw std::runtime_error("Native presentation sequence changed");
        write_value(uint8_t(2)); write_value(index); ++expected;
    } }
}
}
extern "C" int pab_media_main(int argc, const char* const* argv) {
    @autoreleasepool {
        signal(SIGPIPE, SIG_IGN);
        try {
            const auto found = pab_mac::displays();
            if (argc == 2 && std::string(argv[1]) == "inventory") { pab_mac::inventory(found); return 0; }
            if (argc == 3 && std::string(argv[1]) == "cursor") {
                const auto id = pab_mac::number(argv[2], UINT32_MAX);
                for (const auto& display : found) if (display.id == id) {
                    pab_mac::cursor_worker(display); return 0;
                }
                throw std::runtime_error("Selected cursor display is unavailable");
            }
            if (argc == 5 && std::string(argv[1]) == "capture") {
                const auto id = pab_mac::number(argv[2], UINT32_MAX);
                for (const auto& display : found) if (display.id == id) {
                    pab_mac::capture(display, pab_mac::number(argv[3], 60), argv[4]); return 0;
                }
                throw std::runtime_error("Selected display is unavailable");
            }
            if (argc >= 6 && argc <= 8 && std::string(argv[1]) == "render") {
                std::string mode(argv[5]);
                const bool lifecycle = mode.size() > 10 && mode.compare(mode.size()-10,10,"-lifecycle") == 0;
                if (lifecycle) mode.resize(mode.size()-10);
                const bool fail_decoder = mode.size() > 13 && mode.compare(mode.size()-13,13,"-fail-decoder") == 0;
                if (fail_decoder) mode.resize(mode.size()-13);
                const bool software = mode.size() > 9 && mode.compare(mode.size()-9,9,"-software") == 0;
                if (software) mode.resize(mode.size()-9);
                if (mode != "stream" && mode != "stream-verify") throw std::runtime_error("Unsupported Mac renderer mode");
                if (lifecycle && mode != "stream-verify") throw std::runtime_error("Lifecycle check requires generated verification mode");
                if (argc >= 7 && std::string(argv[6]) != "0") throw std::runtime_error("Mac views must be embedded in their owning process");
                unsigned rotation = 0;
                if (argc == 8) {
                    const std::string angle(argv[7]);
                    if (angle == "90") rotation = 1;
                    else if (angle == "180") rotation = 2;
                    else if (angle == "270") rotation = 3;
                    else if (angle != "0") throw std::runtime_error("Invalid rotation");
                }
                pab_mac::render(pab_mac::number(argv[2],8192), pab_mac::number(argv[3],8192),
                    pab_mac::number(argv[4],60), rotation, mode == "stream-verify", software, fail_decoder, lifecycle); return 0;
            }
            throw std::runtime_error("usage: media inventory | capture SCREEN_ID FPS desktop|synthetic | render WIDTH HEIGHT FPS stream|stream-verify");
        } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); return 1; }
        catch (...) { fprintf(stderr, "media: unknown native failure\n"); return 1; }
    }
}
#ifdef PAB_MEDIA_STANDALONE
int main(int argc, const char* const* argv) { return pab_media_main(argc, argv); }
#endif
