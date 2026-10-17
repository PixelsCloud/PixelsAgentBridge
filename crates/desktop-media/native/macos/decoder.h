#pragma once
#include "common.h"
#include "../software_codec_ffi.h"

namespace pab_mac {
struct Nal { const uint8_t* data; size_t size; };
inline std::vector<Nal> annex_b_nals(const std::vector<uint8_t>& bytes) {
    if (bytes.empty() || bytes.size() > max_packet) throw std::runtime_error("Invalid H264 packet size");
    const auto start_code = [&](size_t from) {
        for (size_t i = from; i + 3 <= bytes.size(); ++i) {
            if (bytes[i] || bytes[i+1]) continue;
            if (bytes[i+2] == 1) return std::make_pair(i, size_t(3));
            if (i + 4 <= bytes.size() && !bytes[i+2] && bytes[i+3] == 1) return std::make_pair(i, size_t(4));
        }
        return std::make_pair(bytes.size(), size_t(0));
    };
    auto prefix = start_code(0);
    if (!prefix.second || std::any_of(bytes.begin(), bytes.begin() + prefix.first, [](uint8_t b) { return b != 0; }))
        throw std::runtime_error("H264 Annex-B prefix missing");
    std::vector<Nal> result;
    while (prefix.second) {
        const auto begin = prefix.first + prefix.second;
        const auto next = start_code(begin);
        auto end = next.first;
        while (end > begin && bytes[end-1] == 0) --end; // Annex-B trailing_zero_8bits
        if (end == begin || bytes[begin] & 0x80 || result.size() >= 256)
            throw std::runtime_error("Invalid H264 NAL unit");
        result.push_back({bytes.data() + begin, end - begin});
        prefix = next;
    }
    return result;
}

inline OwnedCF<CMVideoFormatDescriptionRef> describe_h264(const std::vector<uint8_t>& sps,
    const std::vector<uint8_t>& pps, unsigned width, unsigned height) {
    if (sps.empty() || pps.empty() || sps.size() > 65536 || pps.size() > 65536)
        throw std::runtime_error("H264 parameter set missing or too large");
    CMVideoFormatDescriptionRef description = nullptr;
    const uint8_t* sets[] = {sps.data(), pps.data()};
    const size_t sizes[] = {sps.size(), pps.size()};
    check(CMVideoFormatDescriptionCreateFromH264ParameterSets(kCFAllocatorDefault, 2, sets, sizes, 4,
        &description), "H264 parameter description");
    OwnedCF<CMVideoFormatDescriptionRef> owned(description);
    const auto dimensions = CMVideoFormatDescriptionGetDimensions(description);
    if (dimensions.width != int32_t(width) || dimensions.height != int32_t(height))
        throw std::runtime_error("H264 dimensions changed; refresh generation");
    const auto matrix = CMFormatDescriptionGetExtension(description, kCMFormatDescriptionExtension_YCbCrMatrix);
    const auto range = CMFormatDescriptionGetExtension(description, kCMFormatDescriptionExtension_FullRangeVideo);
    if ((matrix && !CFEqual(matrix, kCMFormatDescriptionYCbCrMatrix_ITU_R_709_2)) || range == kCFBooleanTrue)
        throw std::runtime_error("H264 color is not limited BT.709");
    return owned;
}

class HardwareDecoder {
    OwnedCF<VTDecompressionSessionRef> session_;
    OwnedCF<CMVideoFormatDescriptionRef> format_;
    std::vector<uint8_t> sps_, pps_;
    unsigned width_, height_, fps_;
    struct Output {
        std::mutex mutex;
        OwnedCF<CVPixelBufferRef> image;
        CMTime expected = kCMTimeInvalid;
        std::string error;
        bool completed = false;
    } output_;
    static void receive(void* context, void*, OSStatus status, VTDecodeInfoFlags flags,
        CVImageBufferRef image, CMTime time, CMTime) noexcept {
        auto& output = *static_cast<Output*>(context);
        std::lock_guard<std::mutex> lock(output.mutex);
        try {
            check(status, "Hardware decode");
            if (output.completed || !image || (flags & kVTDecodeInfo_FrameDropped) ||
                !CMTIME_IS_NUMERIC(time) || CMTimeCompare(time, output.expected) != 0)
                throw std::runtime_error("Decoder dropped, duplicated or reordered a frame");
            output.image = OwnedCF<CVPixelBufferRef>(CVPixelBufferRetain(image));
        } catch (const std::exception& error) { output.error = error.what(); }
        catch (...) { output.error = "Unknown decoder failure"; }
        output.completed = true;
    }
    void configure(const std::vector<uint8_t>& sps, const std::vector<uint8_t>& pps) {
        auto owned = describe_h264(sps, pps, width_, height_);
        if (session_.get()) { VTDecompressionSessionInvalidate(session_.get()); session_ = OwnedCF<VTDecompressionSessionRef>(); }
        format_ = std::move(owned);
        NSDictionary* specification = @{(__bridge NSString*)kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder: @YES};
        NSDictionary* attributes = @{
            (__bridge NSString*)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
            (__bridge NSString*)kCVPixelBufferMetalCompatibilityKey: @YES,
            (__bridge NSString*)kCVPixelBufferIOSurfacePropertiesKey: @{}};
        VTDecompressionOutputCallbackRecord callback{receive, &output_};
        VTDecompressionSessionRef session = nullptr;
        check(VTDecompressionSessionCreate(kCFAllocatorDefault, format_.get(), (__bridge CFDictionaryRef)specification,
            (__bridge CFDictionaryRef)attributes, &callback, &session), "Hardware decoder create");
        session_ = OwnedCF<VTDecompressionSessionRef>(session);
        check(VTSessionSetProperty(session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue), "Decoder real time");
        sps_ = sps; pps_ = pps;
    }
public:
    HardwareDecoder(unsigned width, unsigned height, unsigned fps) : width_(width), height_(height), fps_(fps) {}
    ~HardwareDecoder() { if (session_.get()) VTDecompressionSessionInvalidate(session_.get()); }
    OwnedCF<CVPixelBufferRef> decode(const std::vector<uint8_t>& packet, uint64_t index) {
        auto sps = sps_, pps = pps_;
        bool idr = false, vcl = false;
        std::vector<uint8_t> avcc;
        for (const auto nal : annex_b_nals(packet)) {
            const unsigned type = nal.data[0] & 31;
            if (type == 7 || type == 8) {
                if (nal.size > 65536) throw std::runtime_error("H264 parameter set too large");
                (type == 7 ? sps : pps).assign(nal.data, nal.data + nal.size);
            } else {
                idr |= type == 5; vcl |= type == 1 || type == 5;
                if (nal.size > max_packet - 4 || avcc.size() > max_packet - 4 - nal.size)
                    throw std::runtime_error("AVCC packet outside bounds");
                const uint32_t size = uint32_t(nal.size);
                avcc.insert(avcc.end(), {uint8_t(size >> 24), uint8_t(size >> 16), uint8_t(size >> 8), uint8_t(size)});
                avcc.insert(avcc.end(), nal.data, nal.data + nal.size);
            }
        }
        if (!vcl) throw std::runtime_error("H264 packet has no frame");
        if (!session_.get() || sps != sps_ || pps != pps_) {
            if (!idr) throw std::runtime_error("Changed H264 format requires an IDR");
            configure(sps, pps);
        }
        CMBlockBufferRef block = nullptr;
        check(CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, nullptr, avcc.size(), kCFAllocatorDefault,
            nullptr, 0, avcc.size(), kCMBlockBufferAssureMemoryNowFlag, &block), "Compressed decode buffer");
        OwnedCF<CMBlockBufferRef> owned_block(block);
        check(CMBlockBufferReplaceDataBytes(avcc.data(), block, 0, avcc.size()), "Compressed decode copy");
        const CMSampleTimingInfo timing{CMTimeMake(1, fps_), CMTimeMake(index, fps_), kCMTimeInvalid};
        const size_t size = avcc.size(); CMSampleBufferRef sample = nullptr;
        check(CMSampleBufferCreateReady(kCFAllocatorDefault, block, format_.get(), 1, 1, &timing, 1, &size, &sample), "Decode sample");
        OwnedCF<CMSampleBufferRef> owned_sample(sample);
        {
            std::lock_guard<std::mutex> lock(output_.mutex);
            output_.completed = false; output_.error.clear(); output_.image = OwnedCF<CVPixelBufferRef>();
            output_.expected = timing.presentationTimeStamp;
        }
        VTDecodeInfoFlags flags{};
        check(VTDecompressionSessionDecodeFrame(session_.get(), sample, 0, nullptr, &flags), "Decode submit");
        check(VTDecompressionSessionWaitForAsynchronousFrames(session_.get()), "Decode completion");
        std::lock_guard<std::mutex> lock(output_.mutex);
        if (!output_.error.empty()) throw std::runtime_error(output_.error);
        if (!output_.completed || !output_.image.get()) throw std::runtime_error("Decoder delayed a low-latency frame");
        const auto image = output_.image.get();
        if (CVPixelBufferGetWidth(image) != width_ || CVPixelBufferGetHeight(image) != height_ ||
            CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange ||
            CVPixelBufferGetPlaneCount(image) != 2 || !CVPixelBufferGetIOSurface(image))
            throw std::runtime_error("Unexpected decoded GPU surface");
        return std::move(output_.image);
    }
};

class VideoDecoder {
    std::unique_ptr<HardwareDecoder> hardware_;
    void* software_ = nullptr;
    OwnedCF<CVPixelBufferPoolRef> pool_;
    std::vector<uint8_t> sps_, pps_;
    unsigned width_, height_, fps_;
    bool started_ = false, force_software_, fail_initial_decoder_;
    void validate(const std::vector<uint8_t>& packet) {
        auto sps = sps_, pps = pps_;
        bool idr = false, vcl = false;
        for (const auto nal : annex_b_nals(packet)) {
            const unsigned type = nal.data[0] & 31;
            if (!type || type >= 24) throw std::runtime_error("Invalid H264 NAL type");
            if (type == 7 || type == 8) {
                if (nal.size > 65536) throw std::runtime_error("H264 parameter set too large");
                (type == 7 ? sps : pps).assign(nal.data, nal.data + nal.size);
            }
            idr |= type == 5; vcl |= type == 1 || type == 5;
        }
        if (!vcl) throw std::runtime_error("H264 packet has no frame");
        if (sps_.empty() || pps_.empty() || sps != sps_ || pps != pps_) {
            if (!idr) throw std::runtime_error("Changed H264 format requires an IDR");
            // Validate before selecting a backend: a corrupt or incompatible
            // stream must not bypass its format contract through fallback.
            describe_h264(sps, pps, width_, height_);
            sps_ = std::move(sps); pps_ = std::move(pps);
        }
    }
    void initialize_software() {
        software_ = pab_soft_decoder_new(width_, height_);
        if (!software_) throw std::runtime_error("Software decoder unavailable");
        NSDictionary* attributes = @{
            (__bridge NSString*)kCVPixelBufferWidthKey: @(width_), (__bridge NSString*)kCVPixelBufferHeightKey: @(height_),
            (__bridge NSString*)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
            (__bridge NSString*)kCVPixelBufferMetalCompatibilityKey: @YES,
            (__bridge NSString*)kCVPixelBufferIOSurfacePropertiesKey: @{}};
        CVPixelBufferPoolRef pool = nullptr;
        check(CVPixelBufferPoolCreate(kCFAllocatorDefault, nullptr, (__bridge CFDictionaryRef)attributes, &pool), "Software texture pool");
        pool_ = OwnedCF<CVPixelBufferPoolRef>(pool);
    }
public:
    VideoDecoder(unsigned width, unsigned height, unsigned fps, bool software, bool fail_initial_decoder = false)
        : width_(width), height_(height), fps_(fps), force_software_(software), fail_initial_decoder_(fail_initial_decoder) {
        const PabVideoGeometry geometry(width, height);
        width_ = geometry.coded_width; height_ = geometry.coded_height;
    }
    ~VideoDecoder() { if (software_) pab_soft_decoder_drop(software_); }
    bool software() const { return software_ != nullptr; }
    OwnedCF<CVPixelBufferRef> decode(const std::vector<uint8_t>& packet, uint64_t index) {
        validate(packet);
        if (!started_) {
            started_ = true;
            if (!force_software_) {
                auto candidate = std::make_unique<HardwareDecoder>(width_, height_, fps_);
                try {
                    auto image = candidate->decode(packet, index);
                    if (fail_initial_decoder_) throw std::runtime_error("Diagnostic initial decoder failure");
                    hardware_ = std::move(candidate); return image;
                } catch (const std::exception& error) {
                    // Retire all partial VT state before selecting the bundled
                    // decoder. After this first frame the path stays immutable.
                    candidate.reset();
                    fprintf(stderr, "media hardware decoder initialization failed: %s; selecting software\n", error.what());
                }
            }
            initialize_software();
        }
        if (hardware_) return hardware_->decode(packet, index);
        PabCodecBytes output{};
        if (!pab_soft_decode(software_, packet.data(), packet.size(), &output))
            throw std::runtime_error("Software decode failed");
        if (!output.data || output.size != size_t(width_)*height_*3/2)
            throw std::runtime_error("Software decoder delayed a frame or changed dimensions");
        CVPixelBufferRef image = nullptr;
        // Four negotiated in-flight frames plus the presenter's retained
        // static image. Keeping only four surfaces can fail when the UI pauses
        // briefly while the background decoder fills its bounded window.
        NSDictionary* allocation = @{(__bridge NSString*)kCVPixelBufferPoolAllocationThresholdKey: @5};
        check(CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(kCFAllocatorDefault, pool_.get(),
            (__bridge CFDictionaryRef)allocation, &image), "Software texture allocation");
        OwnedCF<CVPixelBufferRef> owned(image);
        check(CVPixelBufferLockBaseAddress(image, 0), "Software texture mapping");
        for (unsigned plane = 0; plane < 2; ++plane) {
            const auto* source = output.data + (plane ? size_t(width_)*height_ : 0);
            auto* destination = static_cast<uint8_t*>(CVPixelBufferGetBaseAddressOfPlane(image, plane));
            const size_t pitch = CVPixelBufferGetBytesPerRowOfPlane(image, plane);
            const unsigned rows = plane ? height_/2 : height_;
            if (!destination || pitch < width_) {
                CVPixelBufferUnlockBaseAddress(image, 0); throw std::runtime_error("Invalid software texture stride");
            }
            for (unsigned row = 0; row < rows; ++row)
                memcpy(destination + row*pitch, source + size_t(row)*width_, width_);
        }
        check(CVPixelBufferUnlockBaseAddress(image, 0), "Software texture unmapping");
        return owned;
    }
};
}
