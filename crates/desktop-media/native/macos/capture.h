#pragma once
#include "common.h"
#import <ScreenCaptureKit/ScreenCaptureKit.h>

namespace pab_mac {
struct CaptureState {
    std::mutex mutex;
    OwnedCF<CVPixelBufferRef> latest;
    Clock::time_point sampled;
    std::string error;
    bool closed = false;
};
}
API_AVAILABLE(macos(12.3))
@interface PABStreamOutput : NSObject <SCStreamOutput, SCStreamDelegate> {
@public
    std::shared_ptr<pab_mac::CaptureState> state;
}
@end
@implementation PABStreamOutput
- (void)stream:(SCStream*)stream didStopWithError:(NSError*)error {
    std::lock_guard<std::mutex> lock(state->mutex);
    state->error = "Screen capture stopped: " + std::string(error.localizedDescription.UTF8String ?: "unknown error");
}
- (void)stream:(SCStream*)stream didOutputSampleBuffer:(CMSampleBufferRef)sample ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen || !sample || !CMSampleBufferIsValid(sample)) return;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
    if (!attachments || !CFArrayGetCount(attachments)) return;
    NSDictionary* properties = (__bridge NSDictionary*)CFArrayGetValueAtIndex(attachments, 0);
    NSNumber* status = properties[SCStreamFrameInfoStatus];
    if (!status || status.integerValue != SCFrameStatusComplete) return;
    CVPixelBufferRef image = CMSampleBufferGetImageBuffer(sample);
    if (!image) return;
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return;
    // Replace the one pending frame. Holding this IOSurface reference is not a
    // CPU pixel copy and prevents the capture pool from recycling it too soon.
    state->latest = pab_mac::OwnedCF<CVPixelBufferRef>(CVPixelBufferRetain(image));
    state->sampled = pab_mac::Clock::now();
}
@end

namespace pab_mac {
struct Completion {
    std::mutex mutex;
    bool done = false;
    std::string error;
    void finish(NSError* value) {
        std::lock_guard<std::mutex> lock(mutex);
        if (value) error = value.localizedDescription.UTF8String ?: "Screen capture error";
        done = true;
    }
};
inline void wait_completion(const std::shared_ptr<Completion>& state, const char* operation) {
    const auto deadline = Clock::now() + std::chrono::seconds(10);
    while (Clock::now() < deadline) {
        {
            std::lock_guard<std::mutex> lock(state->mutex);
            if (state->done) {
                if (!state->error.empty()) throw std::runtime_error(std::string(operation) + ": " + state->error);
                return;
            }
        }
        parent_alive(); pump();
    }
    throw std::runtime_error(std::string(operation) + " timed out");
}
class API_AVAILABLE(macos(12.3)) Capture {
    Display display_;
    std::shared_ptr<CaptureState> state_ = std::make_shared<CaptureState>();
    SCStream* __strong stream_ = nil;
    PABStreamOutput* __strong output_ = nil;
    dispatch_queue_t queue_;
public:
    Capture(const Display& display, unsigned fps, bool embedded_cursor = true) : display_(display) {
        if (!CGPreflightScreenCaptureAccess())
            throw std::runtime_error("Screen Recording permission is required for this application");
        const auto content_ready = std::make_shared<Completion>();
        // The callback can outlive a timeout; its captured storage is owned.
        struct Shareable { SCShareableContent* __strong value = nil; };
        const auto shareable = std::make_shared<Shareable>();
        [SCShareableContent getShareableContentExcludingDesktopWindows:NO onScreenWindowsOnly:YES
            completionHandler:^(SCShareableContent* content, NSError* error) {
                shareable->value = content; content_ready->finish(error);
            }];
        wait_completion(content_ready, "Enumerate capture displays");
        SCDisplay* selected = nil;
        for (SCDisplay* candidate in shareable->value.displays)
            if (candidate.displayID == display.id) { selected = candidate; break; }
        if (!selected) throw std::runtime_error("Selected display disappeared");
        SCContentFilter* filter = [[SCContentFilter alloc] initWithDisplay:selected excludingWindows:@[]];
        SCStreamConfiguration* config = [[SCStreamConfiguration alloc] init];
        config.width = display.width; config.height = display.height;
        config.minimumFrameInterval = CMTimeMake(1, fps);
        config.queueDepth = 3;
        config.showsCursor = embedded_cursor;
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
        config.colorMatrix = kCGDisplayStreamYCbCrMatrix_ITU_R_709_2;
        config.colorSpaceName = kCGColorSpaceITUR_709;
        output_ = [[PABStreamOutput alloc] init]; output_->state = state_;
        queue_ = dispatch_queue_create("pixels.media.capture", DISPATCH_QUEUE_SERIAL);
        stream_ = [[SCStream alloc] initWithFilter:filter configuration:config delegate:output_];
        NSError* error = nil;
        if (![stream_ addStreamOutput:output_ type:SCStreamOutputTypeScreen sampleHandlerQueue:queue_ error:&error])
            throw std::runtime_error("Cannot register capture output");
        const auto ready = std::make_shared<Completion>();
        [stream_ startCaptureWithCompletionHandler:^(NSError* error) { ready->finish(error); }];
        wait_completion(ready, "Start screen capture");
    }
    ~Capture() {
        { std::lock_guard<std::mutex> lock(state_->mutex); state_->closed = true; state_->latest = OwnedCF<CVPixelBufferRef>(); }
        [stream_ stopCaptureWithCompletionHandler:^(NSError*) {}];
        // The delegate retains shared state until all queued callbacks retire.
    }
    OwnedCF<CVPixelBufferRef> next(Clock::time_point& sampled) {
        for (;;) {
            parent_alive();
            if (!CGDisplayIsActive(display_.id) || !CGRectEqualToRect(CGDisplayBounds(display_.id), display_.bounds))
                throw std::runtime_error("Screen changed; refresh generation");
            {
                std::lock_guard<std::mutex> lock(state_->mutex);
                if (!state_->error.empty()) throw std::runtime_error(state_->error);
                if (state_->latest.get()) {
                    if (CVPixelBufferGetWidth(state_->latest.get()) != display_.width ||
                        CVPixelBufferGetHeight(state_->latest.get()) != display_.height ||
                        CVPixelBufferGetPixelFormatType(state_->latest.get()) != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange ||
                        !CVPixelBufferGetIOSurface(state_->latest.get()))
                        throw std::runtime_error("Capture texture changed; refresh generation");
                    sampled = state_->sampled;
                    return std::move(state_->latest);
                }
            }
            pump();
        }
    }
};
}
