// Run on an Apple hardware encoder. Inject only the documented successful
// drop notifications; real frames before/after still use VideoToolbox.
#import <VideoToolbox/VideoToolbox.h>
#include <cassert>
#include <cstdio>
#include <cstring>
static VTCompressionOutputCallback callback;
static void* context;
static unsigned operation;
static CMTime previous = kCMTimeInvalid;
static OSStatus create_session(CFAllocatorRef allocator, int32_t width, int32_t height,
    CMVideoCodecType codec, CFDictionaryRef spec, CFDictionaryRef attrs, CFAllocatorRef compressed,
    VTCompressionOutputCallback output, void* refcon, VTCompressionSessionRef* session) {
    callback = output; context = refcon;
    return VTCompressionSessionCreate(allocator, width, height, codec, spec, attrs, compressed, output, refcon, session);
}
static OSStatus encode_frame(VTCompressionSessionRef session, CVImageBufferRef image, CMTime pts, CMTime duration,
    CFDictionaryRef properties, void* refcon, VTEncodeInfoFlags* flags) {
    assert(!CMTIME_IS_VALID(previous) || CMTimeCompare(pts, previous) > 0);
    previous = pts;
    if (operation == 1) { *flags = kVTEncodeInfo_FrameDropped; return noErr; }
    if (operation == 2) { callback(context, refcon, noErr, kVTEncodeInfo_FrameDropped, nullptr); return noErr; }
    if (operation == 3) return paramErr;
    return VTCompressionSessionEncodeFrame(session, image, pts, duration, properties, refcon, flags);
}
#define VTCompressionSessionCreate create_session
#define VTCompressionSessionEncodeFrame encode_frame
#include "../native/macos/encoder.h"

int main() { @autoreleasepool {
    try {
        pab_mac::Encoder encoder(256, 256, 15);
        CVPixelBufferRef raw = nullptr;
        const auto attrs = @{(__bridge NSString*)kCVPixelBufferIOSurfacePropertiesKey: @{}};
        pab_mac::check(CVPixelBufferCreate(nullptr, 256, 256, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            (__bridge CFDictionaryRef)attrs, &raw), "Create generated test image");
        pab_mac::OwnedCF<CVPixelBufferRef> image(raw);
        pab_mac::check(CVPixelBufferLockBaseAddress(raw, 0), "Map generated image");
        for (size_t i = 0; i < 2; ++i)
            memset(CVPixelBufferGetBaseAddressOfPlane(raw, i), i ? 128 : 90,
                CVPixelBufferGetBytesPerRowOfPlane(raw, i) * CVPixelBufferGetHeightOfPlane(raw, i));
        CVPixelBufferUnlockBaseAddress(raw, 0);
        assert(encoder.encode(raw, 0).key);
        for (operation = 1; operation <= 2; ++operation) {
            const auto frame = encoder.encode(raw, 1);
            assert(frame.bytes.empty() && !frame.key);
        }
        operation = 0;
        assert(!encoder.encode(raw, 1).bytes.empty());
        operation = 3; bool failed = false;
        try { encoder.encode(raw, 2); } catch (const std::exception&) { failed = true; }
        assert(failed);
        puts("{\"synchronous_drop\":true,\"callback_drop\":true,\"submission_pts_increase\":true,\"real_encode_recovers\":true,\"real_status_error_preserved\":true}");
        return 0;
    } catch (const std::exception& error) {
        fprintf(stderr, "%s\n", error.what()); return 2;
    }
} }
