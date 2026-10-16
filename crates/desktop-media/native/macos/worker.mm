// Native macOS worker: compressed frames use the existing private pipe and
// authenticated reliable transport. No application/network configuration here.
#include "encoder.h"
#include "capture.h"
#include <csignal>

namespace pab_mac {
// Opt-in diagnostic only. Actual screen capture never calls LockBaseAddress.
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
    if (mode != "desktop" && mode != "synthetic") throw std::runtime_error("Unknown capture mode");
    const auto start = Clock::now(); auto next = start;
    Encoder encoder(display.width, display.height, fps);
    std::function<OwnedCF<CVPixelBufferRef>(Clock::time_point&)> source;
    if (mode == "desktop") {
        if (@available(macOS 12.3, *)) {
            const auto capture = std::make_shared<Capture>(display, fps);
            source = [capture](Clock::time_point& time) { return capture->next(time); };
        }
        else throw std::runtime_error("Remote screen capture requires macOS 12.3 or later");
    }
    fprintf(stderr, "media encoder=videotoolbox_hardware source=%s cpu_pixel_mappings_per_frame=%u\n",
        mode.c_str(), mode == "desktop" ? 0 : 1);
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
}
extern "C" int pab_media_main(int argc, const char* const* argv) {
    @autoreleasepool {
        signal(SIGPIPE, SIG_IGN);
        try {
            const auto found = pab_mac::displays();
            if (argc == 2 && std::string(argv[1]) == "inventory") { pab_mac::inventory(found); return 0; }
            if (argc == 5 && std::string(argv[1]) == "capture") {
                const auto id = pab_mac::number(argv[2], UINT32_MAX);
                for (const auto& display : found) if (display.id == id) {
                    pab_mac::capture(display, pab_mac::number(argv[3], 60), argv[4]); return 0;
                }
                throw std::runtime_error("Selected display is unavailable");
            }
            throw std::runtime_error("usage: media inventory | capture SCREEN_ID FPS desktop|synthetic");
        } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); return 1; }
        catch (...) { fprintf(stderr, "media: unknown native failure\n"); return 1; }
    }
}
#ifdef PAB_MEDIA_STANDALONE
int main(int argc, const char* const* argv) { return pab_media_main(argc, argv); }
#endif
