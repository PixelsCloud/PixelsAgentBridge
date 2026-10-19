#pragma once
#include "common.h"
#include "../software_codec_ffi.h"

namespace pab_mac {
struct EncodedFrame { std::vector<uint8_t> bytes; bool key = false; };
class Encoder {
    OwnedCF<VTCompressionSessionRef> session_;
    unsigned fps_;
    uint64_t original_budget_;
    struct Output {
        std::mutex mutex;
        bool completed = false;
        EncodedFrame frame;
        std::string error;
    } output_;
    static void append_nal(std::vector<uint8_t>& result, const uint8_t* data, size_t size) {
        if (!size || size > max_packet - 4 || result.size() > max_packet - size - 4)
            throw std::runtime_error("Encoded packet outside bounds");
        result.insert(result.end(), {0, 0, 0, 1});
        result.insert(result.end(), data, data + size);
    }
    static EncodedFrame annex_b(CMSampleBufferRef sample) {
        if (!sample || !CMSampleBufferDataIsReady(sample) || CMSampleBufferGetNumSamples(sample) != 1)
            throw std::runtime_error("Invalid encoder sample");
        EncodedFrame result;
        const auto attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
        CFDictionaryRef properties = attachments && CFArrayGetCount(attachments)
            ? static_cast<CFDictionaryRef>(CFArrayGetValueAtIndex(attachments, 0)) : nullptr;
        result.key = !properties || CFDictionaryGetValue(properties, kCMSampleAttachmentKey_NotSync) != kCFBooleanTrue;
        const auto format = CMSampleBufferGetFormatDescription(sample);
        int length_bytes = 0; size_t sets = 0;
        check(CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, 0, nullptr, nullptr, &sets, &length_bytes), "H264 format");
        if (length_bytes != 4 || sets < 2 || sets > 16) throw std::runtime_error("Unsupported H264 format");
        if (result.key) {
            for (size_t i = 0; i < sets; ++i) {
                const uint8_t* data = nullptr; size_t size = 0;
                check(CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, i, &data, &size, nullptr, nullptr), "H264 parameter set");
                append_nal(result.bytes, data, size);
            }
        }
        CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sample);
        if (!block) throw std::runtime_error("Encoder block missing");
        const size_t total = CMBlockBufferGetDataLength(block);
        if (!total || total > max_packet) throw std::runtime_error("Encoded block outside bounds");
        // Copy only compressed bytes. Neither capture nor encoding maps pixels.
        std::vector<uint8_t> avcc(total);
        check(CMBlockBufferCopyDataBytes(block, 0, total, avcc.data()), "Encoder block read");
        size_t offset = 0;
        while (offset < total) {
            if (total - offset < 4) throw std::runtime_error("Truncated H264 NAL length");
            const uint32_t size = (uint32_t(avcc[offset]) << 24) | (uint32_t(avcc[offset+1]) << 16) |
                (uint32_t(avcc[offset+2]) << 8) | avcc[offset+3];
            offset += 4;
            if (!size || size > total - offset) throw std::runtime_error("Truncated H264 NAL");
            append_nal(result.bytes, avcc.data() + offset, size); offset += size;
        }
        return result;
    }
    static void receive(void* context, void*, OSStatus status, VTEncodeInfoFlags flags, CMSampleBufferRef sample) noexcept {
        auto& output = *static_cast<Output*>(context);
        std::lock_guard<std::mutex> lock(output.mutex);
        try {
            check(status, "Hardware encode");
            if (output.completed) throw std::runtime_error("Unexpected duplicate encoder output");
            if (flags & kVTEncodeInfo_FrameDropped) throw std::runtime_error("Encoder dropped a requested frame");
            output.frame = annex_b(sample);
        } catch (const std::exception& error) { output.error = error.what(); }
        catch (...) { output.error = "Unknown encoder failure"; }
        output.completed = true;
    }
    void property(CFStringRef key, CFTypeRef value) {
        const std::string operation = "Encoder property " + std::string([(__bridge NSString*)key UTF8String]);
        check(VTSessionSetProperty(session_.get(), key, value), operation.c_str());
    }
public:
    Encoder(unsigned width, unsigned height, unsigned fps) : fps_(fps),
        original_budget_(std::max(uint64_t(2000000), uint64_t(width)*height*fps/4)) {
        if (!width || !height || width > 8192 || height > 8192 || uint64_t(width)*height > 33554432 ||
            width % 2 || height % 2 || !fps || fps > 60) throw std::runtime_error("Encoder geometry outside bounds");
        NSDictionary* specification = @{
            (__bridge NSString*)kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: @YES,
            (__bridge NSString*)kVTVideoEncoderSpecification_EnableLowLatencyRateControl: @YES};
        NSDictionary* attributes = @{(__bridge NSString*)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
            (__bridge NSString*)kCVPixelBufferIOSurfacePropertiesKey: @{}};
        VTCompressionSessionRef session = nullptr;
        check(VTCompressionSessionCreate(kCFAllocatorDefault, width, height, kCMVideoCodecType_H264,
            (__bridge CFDictionaryRef)specification, (__bridge CFDictionaryRef)attributes,
            nullptr, receive, &output_, &session), "Hardware encoder create");
        session_ = OwnedCF<VTCompressionSessionRef>(session);
        property(kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
        property(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
        property(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel);
        property(kVTCompressionPropertyKey_BaseLayerFrameRateFraction, (__bridge CFNumberRef)@1.0);
        property(kVTCompressionPropertyKey_EnableLTR, kCFBooleanFalse);
        // One reference plus explicit no-reorder VUI is interoperable with
        // both the bounded Windows hardware pipeline and bundled OpenH264.
        // Older macOS needs a separately verified fallback before advertising
        // this backend; silently accepting its default multi-reference stream
        // can produce stale software-decoded images.
        if (@available(macOS 13.0, *))
            property(kVTCompressionPropertyKey_ReferenceBufferCount, (__bridge CFNumberRef)@1);
        else throw std::runtime_error("This hardware media backend requires macOS 13 or later");
        property(kVTCompressionPropertyKey_ExpectedFrameRate, (__bridge CFNumberRef)@(fps));
        property(kVTCompressionPropertyKey_MaxKeyFrameInterval, (__bridge CFNumberRef)@(fps * 2));
        property(kVTCompressionPropertyKey_AverageBitRate, (__bridge CFNumberRef)@(original_budget_));
        property(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
        property(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
        property(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
        check(VTCompressionSessionPrepareToEncodeFrames(session_.get()), "Encoder prepare");
        CFTypeRef hardware = nullptr;
        const auto hardware_status = VTSessionCopyProperty(session_.get(), kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
            kCFAllocatorDefault, &hardware);
        OwnedCF<CFTypeRef> used(hardware);
        // Apple's low-latency encoder does not expose this informational
        // property on every system. RequireHardware in session creation still
        // prohibits a software encoder; honor an explicit false result.
        if (hardware_status != kVTPropertyNotSupportedErr) {
            check(hardware_status, "Encoder hardware check");
            if (hardware != kCFBooleanTrue) throw std::runtime_error("Hardware encoder was not selected");
        }
    }
    ~Encoder() {
        // Stop callbacks while output_ still exists. CFRelease alone is not a
        // callback lifetime barrier, including constructor/encode failures.
        if (session_.get()) VTCompressionSessionInvalidate(session_.get());
    }
    void budget(uint32_t value) {
        if (value && (value < 150000 || value > 16000000)) throw std::runtime_error("Encoder budget outside bounds");
        const auto target = value ? std::min(uint64_t(value), original_budget_) : original_budget_;
        property(kVTCompressionPropertyKey_AverageBitRate, (__bridge CFNumberRef)@(target));
    }
    EncodedFrame encode(CVPixelBufferRef image, uint64_t index) {
        {
            std::lock_guard<std::mutex> lock(output_.mutex);
            output_.completed = false; output_.frame = {}; output_.error.clear();
        }
        NSDictionary* options = index == 0 ? @{(__bridge NSString*)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES} : nil;
        VTEncodeInfoFlags flags{};
        check(VTCompressionSessionEncodeFrame(session_.get(), image, CMTimeMake(index, fps_), CMTimeMake(1, fps_),
            (__bridge CFDictionaryRef)options, nullptr, &flags), "Encode frame submit");
        // One requested frame in flight: no hidden encoder backlog is allowed
        // to accumulate behind the reliable transport's existing credit window.
        check(VTCompressionSessionCompleteFrames(session_.get(), CMTimeMake(index, fps_)), "Encode frame complete");
        std::lock_guard<std::mutex> lock(output_.mutex);
        if (!output_.error.empty()) throw std::runtime_error(output_.error);
        if (!output_.completed || output_.frame.bytes.empty() || (index == 0 && !output_.frame.key))
            throw std::runtime_error("Encoder did not produce the requested frame");
        return std::move(output_.frame);
    }
};

// The capture IOSurface goes directly to VT while hardware works. Only the
// bundled fallback maps its NV12 planes; it never round-trips through RGB.
class VideoEncoder {
    std::unique_ptr<Encoder> hardware_;
    void* software_ = nullptr;
    PabVideoGeometry geometry_;
    unsigned fps_;
    bool fail_encode_;
    uint32_t budget_ = 0;
    void use_software() {
        hardware_.reset();
        software_ = pab_soft_encoder_new(geometry_.width, geometry_.height, static_cast<uint16_t>(fps_));
        if (!software_) throw std::runtime_error("Software encoder initialization failed");
        if (budget_ && !pab_soft_encoder_budget(software_, budget_))
            throw std::runtime_error("Software encoder budget initialization failed");
    }
public:
    VideoEncoder(unsigned width, unsigned height, unsigned fps, bool software = false,
        bool fail_initial = false, bool fail_encode = false)
        : geometry_(width, height), fps_(fps), fail_encode_(fail_encode) {
        if (!fps || fps > 60) throw std::runtime_error("Invalid encoder frame rate");
        if (!software) {
            try {
                hardware_ = std::make_unique<Encoder>(width, height, fps);
                if (fail_initial) throw std::runtime_error("Diagnostic encoder initialization failure");
                return;
            } catch (const std::exception& error) {
                fprintf(stderr, "media hardware encoder initialization failed: %s; selecting software\n", error.what());
            }
        }
        use_software();
    }
    ~VideoEncoder() { if (software_) pab_soft_encoder_drop(software_); }
    bool software() const { return software_ != nullptr; }
    bool budget(uint32_t value) {
        if (value && (value < 150000 || value > 16000000)) throw std::runtime_error("Encoder budget outside bounds");
        if (hardware_) {
            try { hardware_->budget(value); budget_ = value; return true; }
            catch (const std::exception& error) {
                fprintf(stderr, "media hardware budget update unsupported: %s; retaining direct IOSurface path\n", error.what());
                return false;
            }
        }
        if (!pab_soft_encoder_budget(software_, value)) throw std::runtime_error("Software encoder budget update failed");
        budget_ = value;
        return true;
    }
    EncodedFrame encode(CVPixelBufferRef image, uint64_t index) {
        if (!image || CVPixelBufferGetWidth(image) != geometry_.width ||
            CVPixelBufferGetHeight(image) != geometry_.height || CVPixelBufferGetPlaneCount(image) != 2 ||
            CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)
            throw std::runtime_error("Encoder input changed; refresh generation");
        if (hardware_) {
            try {
                auto frame = hardware_->encode(image, index);
                if (fail_encode_ && index == 2) throw std::runtime_error("Diagnostic running encoder failure");
                return frame;
            } catch (const std::exception& error) {
                fprintf(stderr, "media hardware encoder failed: %s; selecting software\n", error.what());
                use_software();
                fprintf(stderr, "media encoder fallback=openh264_software cpu_pixel_mappings_per_frame=1\n");
            }
        }
        const size_t y_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 0);
        const size_t uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image, 1);
        const size_t y_rows = CVPixelBufferGetHeightOfPlane(image, 0);
        const size_t uv_rows = CVPixelBufferGetHeightOfPlane(image, 1);
        if (y_rows != geometry_.height || uv_rows != geometry_.coded_height / 2 ||
            y_stride < geometry_.width || uv_stride < geometry_.coded_width ||
            y_stride > (256u * 1024 * 1024) / y_rows || uv_stride > (256u * 1024 * 1024) / uv_rows)
            throw std::runtime_error("Encoder NV12 plane bounds changed");
        check(CVPixelBufferLockBaseAddress(image, kCVPixelBufferLock_ReadOnly), "Software capture mapping");
        PabCodecBytes output{};
        const bool ok = pab_soft_encode_nv12(software_,
            static_cast<const uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image, 0)), y_stride*y_rows, y_stride,
            static_cast<const uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image, 1)), uv_stride*uv_rows, uv_stride, &output);
        check(CVPixelBufferUnlockBaseAddress(image, kCVPixelBufferLock_ReadOnly), "Software capture unmapping");
        if (!ok || !output.data || !output.size || output.size > max_packet)
            throw std::runtime_error("Software encode failed");
        EncodedFrame frame;
        frame.bytes.assign(output.data, output.data + output.size);
        for (size_t i = 0; i + 4 < frame.bytes.size(); ++i) {
            if (frame.bytes[i] == 0 && frame.bytes[i+1] == 0 && frame.bytes[i+2] == 0 &&
                frame.bytes[i+3] == 1 && (frame.bytes[i+4] & 31) == 5) frame.key = true;
        }
        return frame;
    }
};
}
