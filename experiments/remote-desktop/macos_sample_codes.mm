// Decode observer packets from the already-authorized screen capture helper.
// This process never captures the screen or stores pixels. It emits only codes.
#include <decoder.h>
#include <cstdio>

int main(int argc, const char* const* argv) {
    @autoreleasepool {
        try {
            if (argc != 8) throw std::runtime_error("WIDTH HEIGHT X Y VIEW_WIDTH VIEW_HEIGHT VIEWER_PID required");
            const unsigned width = pab_mac::number(argv[1], 8192), height = pab_mac::number(argv[2], 8192);
            auto coordinate = [](const char* text) {
                size_t end = 0; const std::string value(text); const auto n = std::stoul(value, &end);
                if (end != value.size() || n > 8192) throw std::runtime_error("Invalid coordinate");
                return unsigned(n);
            };
            const unsigned x = coordinate(argv[3]), y = coordinate(argv[4]);
            const unsigned w = pab_mac::number(argv[5], 8192), h = pab_mac::number(argv[6], 8192);
            const unsigned owner = pab_mac::number(argv[7], UINT32_MAX);
            if (x + w > width || y + h > height) throw std::runtime_error("View outside screen");
            // Acceptance currently requires a single 1x primary screen. Bounds
            // are independently checked, without relying on private window titles.
            const auto screen = CGMainDisplayID();
            if (CGDisplayBounds(screen).origin.x != 0 || CGDisplayBounds(screen).origin.y != 0 ||
                CGDisplayBounds(screen).size.width != width || CGDisplayBounds(screen).size.height != height ||
                CGDisplayPixelsWide(screen) != width || CGDisplayPixelsHigh(screen) != height)
                throw std::runtime_error("Sampler requires the verified 1x primary display");
            CGRect original = CGRectNull; unsigned window_id = 0;
            auto check_window = [&] {
                pab_mac::OwnedCF<CFArrayRef> list(CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID));
                unsigned matches = 0;
                for (NSDictionary* item in (__bridge NSArray*)list.get()) {
                    if ([item[(__bridge NSString*)kCGWindowOwnerPID] unsignedIntValue] != owner) continue;
                    if ([item[(__bridge NSString*)kCGWindowLayer] intValue] != 0) continue;
                    CGRect bounds{};
                    if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)item[(__bridge NSString*)kCGWindowBounds], &bounds)) continue;
                    if (bounds.size.width != w || !CGRectContainsRect(bounds, CGRectMake(x, y, w, h))) continue;
                    const unsigned id = [item[(__bridge NSString*)kCGWindowNumber] unsignedIntValue];
                    if (CGRectIsNull(original)) { original = bounds; window_id = id; }
                    if (id != window_id || !CGRectEqualToRect(original, bounds)) throw std::runtime_error("Viewer changed during sampling");
                    ++matches;
                }
                if (matches != 1) throw std::runtime_error("Owned viewer unavailable or ambiguous");
            };
            check_window();
            const double scale = std::min(w / 1920.0, h / 1080.0);
            const double ox = x + (w - 1920 * scale) / 2, oy = y + (h - 1080 * scale) / 2;
            pab_mac::HardwareDecoder decoder(width, height, 60);
            uint64_t expected = 0;
            for (;;) {
                const auto size = pab_mac::read_value<uint32_t>();
                if (!size) return 0;
                if (size > pab_mac::max_packet) throw std::runtime_error("Oversize observer packet");
                const auto index = pab_mac::read_value<uint64_t>();
                const auto time = pab_mac::read_value<uint64_t>();
                if (index != expected++) throw std::runtime_error("Observer packet reordered");
                if (index % 15 == 0) check_window();
                std::vector<uint8_t> packet(size); pab_mac::read_bytes(packet.data(), size);
                auto image = decoder.decode(packet, index);
                pab_mac::check(CVPixelBufferLockBaseAddress(image.get(), kCVPixelBufferLock_ReadOnly), "Observer pixel mapping");
                const auto* luma = static_cast<const uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image.get(), 0));
                const auto stride = CVPixelBufferGetBytesPerRowOfPlane(image.get(), 0);
                int bits[32]; bool valid = true;
                for (unsigned row = 0; row < 2; ++row) for (unsigned i = 0; i < 16; ++i) {
                    const auto px = size_t(std::lround(ox + (240 + i * 96) * scale));
                    const auto py = size_t(std::lround(oy + (200 + row * 96) * scale));
                    const auto value = luma[py * stride + px];
                    bits[row * 16 + i] = value > 200 ? 1 : value < 50 ? 0 : -1;
                    valid = valid && bits[row * 16 + i] >= 0;
                }
                CVPixelBufferUnlockBaseAddress(image.get(), kCVPixelBufferLock_ReadOnly);
                for (unsigned i = 0; i < 16; ++i) valid = valid && bits[i] != bits[16 + i];
                valid = valid && bits[0] == 1 && bits[1] == 0 && bits[14] == 0 && bits[15] == 1;
                unsigned code = 0;
                for (unsigned i = 0; i < 12; ++i) if (bits[i + 2] == 1) code |= 1U << i;
                printf("{\"sample_us\":%llu,\"index\":%llu,\"code\":%d}\n",
                    static_cast<unsigned long long>(time), static_cast<unsigned long long>(index), valid ? int(code) : -1);
                fflush(stdout);
            }
        } catch (const std::exception& e) { fprintf(stderr, "%s\n", e.what()); return 1; }
    }
}
