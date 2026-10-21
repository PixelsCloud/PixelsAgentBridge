#pragma once
#include "common.h"
#include "viewer_input.h"
#include "viewer_stats.h"
#include "cursor_overlay.h"
#include "video_shader.h"
#include "../software_codec_ffi.h"
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#import <QuartzCore/CATransaction.h>
#import <CoreVideo/CVMetalTextureCache.h>
#include <array>
#include <optional>

@interface PABMetalView : PABInputView
@property(nonatomic) BOOL needsMediaRedraw;
@property(nonatomic) BOOL tracksRemoteCursor;
@property(nonatomic, strong) PABViewerStats* statisticsOverlay;
@end
@implementation PABMetalView
- (void)resetCursorRects {
    [super resetCursorRects];
    if (!self.tracksRemoteCursor || !mediaInput.enabled) return;
    const double w = mediaInput.rotation%2 ? mediaInput.height : mediaInput.width;
    const double h = mediaInput.rotation%2 ? mediaInput.width : mediaInput.height;
    if (w <= 0 || h <= 0) return;
    const auto bounds = self.bounds;
    const double scale = std::min(bounds.size.width/w,bounds.size.height/h);
    if (scale <= 0) return;
    static NSCursor* transparent = [] {
        auto bitmap = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:nullptr pixelsWide:1 pixelsHigh:1
            bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:4 bitsPerPixel:32];
        if (!bitmap) return NSCursor.arrowCursor;
        std::memset(bitmap.bitmapData,0,4);
        auto image = [[NSImage alloc] initWithSize:NSMakeSize(1,1)];
        [image addRepresentation:bitmap];
        return [[NSCursor alloc] initWithImage:image hotSpot:NSZeroPoint];
    }();
    [self addCursorRect:NSMakeRect(NSMidX(bounds)-w*scale/2,NSMidY(bounds)-h*scale/2,w*scale,h*scale) cursor:transparent];
}
- (void)setMediaInputEnabled:(BOOL)enabled {
    [super setMediaInputEnabled:enabled];
    [self.window invalidateCursorRectsForView:self];
}
- (CALayer*)makeBackingLayer { return [CAMetalLayer layer]; }
- (void)setFrameSize:(NSSize)size { [super setFrameSize:size]; self.needsMediaRedraw = YES; }
- (void)viewDidChangeBackingProperties { [super viewDidChangeBackingProperties]; self.needsMediaRedraw = YES; }
- (void)windowMediaChanged:(NSNotification*)notification {
    if ([notification.name isEqualToString:NSWindowDidResignKeyNotification] ||
        [notification.name isEqualToString:NSWindowDidMiniaturizeNotification]) mediaInput.release();
    self.needsMediaRedraw = YES;
}
- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    [NSNotificationCenter.defaultCenter removeObserver:self];
    for (NSNotificationName name in @[NSWindowDidDeminiaturizeNotification, NSWindowDidChangeOcclusionStateNotification,
        NSWindowDidResignKeyNotification, NSWindowDidMiniaturizeNotification])
        if (self.window) [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(windowMediaChanged:) name:name object:self.window];
    self.needsMediaRedraw = YES;
}
- (void)dealloc { [NSNotificationCenter.defaultCenter removeObserver:self]; }
- (void)layout {
    [super layout];
    [self.statisticsOverlay placeInViewer];
    CAMetalLayer* metal = (CAMetalLayer*)self.layer;
    const NSRect backing = [self convertRectToBacking:self.bounds];
    metal.contentsScale = self.window.backingScaleFactor ?: 1.0;
    metal.drawableSize = CGSizeMake(std::max(1.0, backing.size.width), std::max(1.0, backing.size.height));
    self.needsMediaRedraw = YES;
}
@end

namespace pab_mac {
inline std::array<unsigned, 3> test_rgb(uint64_t frame, unsigned quadrant) {
    return {unsigned(24 + (frame * 3 + quadrant * 43) % 208), unsigned(24 + (frame * 7 + quadrant * 67) % 208),
        unsigned(24 + (frame * 11 + quadrant * 97) % 208)};
}
struct DisplayStats {
    std::mutex mutex;
    uint64_t displayed = 0, last_index = 0, callbacks = 0, zero_timestamps = 0;
    uint64_t result_submission = 0, confirmed_submission = 0;
    std::optional<uint64_t> dropped;
    bool seen = false;
};
struct TextureLease {
    OwnedCF<CVPixelBufferRef> image;
    OwnedCF<CVMetalTextureRef> y, uv;
};

// UI creation, layout and draws are main-thread operations. This view can be
// attached to a Desktop-owned NSView in-process; no NSView address crosses IPC.
class NativePresenter {
    NSWindow* __strong window_ = nil;
    PABMetalView* __strong view_ = nil;
    id<MTLDevice> __strong device_ = nil;
    id<MTLCommandQueue> __strong queue_ = nil;
    id<MTLRenderPipelineState> __strong pipeline_ = nil;
    OwnedCF<CVMetalTextureCacheRef> cache_;
    OwnedCF<CVPixelBufferRef> last_image_;
    uint64_t last_index_ = 0;
    uint64_t submission_ = 0;
    unsigned width_, height_, rotation_;
    PabVideoGeometry geometry_;
    CursorOverlay cursor_;
    bool verify_, owns_window_ = false, last_verified_ = false;
    Clock::time_point next_redraw_{};
    std::shared_ptr<DisplayStats> stats_ = std::make_shared<DisplayStats>();
    static void require_main() {
        if (![NSThread isMainThread]) throw std::runtime_error("Native view requires the AppKit main thread");
    }
    bool draw(CVPixelBufferRef image, uint64_t index, bool verify) {
        require_main();
        if (view_.window.miniaturized || !view_.window.visible || NSIsEmptyRect(view_.bounds) ||
            !(view_.window.occlusionState & NSWindowOcclusionStateVisible)) return false;
        [view_ layoutSubtreeIfNeeded]; view_.needsMediaRedraw = NO;
        CAMetalLayer* layer = (CAMetalLayer*)view_.layer;
        const NSRect backing = [view_ convertRectToBacking:view_.bounds];
        const auto size = CGSizeMake(std::max(1.0, backing.size.width), std::max(1.0, backing.size.height));
        if (!CGSizeEqualToSize(layer.drawableSize, size)) layer.drawableSize = size;
        // Commit attachment/layout before presenting the first drawable. A
        // static stream may never provide a second frame to repair a drawable
        // discarded while its layer was not yet attached to WindowServer.
        [CATransaction flush];
        const auto drawing = Clock::now();
        id<CAMetalDrawable> drawable = [layer nextDrawable];
        drawable_us += std::chrono::duration<double,std::micro>(Clock::now()-drawing).count();
        if (!drawable) throw std::runtime_error("Metal drawable unavailable");
        auto lease = std::make_shared<TextureLease>();
        lease->image = OwnedCF<CVPixelBufferRef>(CVPixelBufferRetain(image));
        CVMetalTextureRef y = nullptr, uv = nullptr;
        check(CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, cache_.get(), image, nullptr,
            MTLPixelFormatR8Unorm, geometry_.coded_width, geometry_.coded_height, 0, &y), "Metal luma view");
        lease->y = OwnedCF<CVMetalTextureRef>(y);
        check(CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault, cache_.get(), image, nullptr,
            MTLPixelFormatRG8Unorm, geometry_.coded_width/2, geometry_.coded_height/2, 1, &uv), "Metal chroma view");
        lease->uv = OwnedCF<CVMetalTextureRef>(uv);
        const double rotated_w = rotation_ % 2 ? height_ : width_, rotated_h = rotation_ % 2 ? width_ : height_;
        const double scale = std::min(drawable.texture.width/rotated_w, drawable.texture.height/rotated_h);
        const MTLViewport viewport{(drawable.texture.width - rotated_w*scale)/2,
            (drawable.texture.height - rotated_h*scale)/2, rotated_w*scale, rotated_h*scale, 0, 1};
        MTLRenderPassDescriptor* pass = [MTLRenderPassDescriptor renderPassDescriptor];
        pass.colorAttachments[0].texture = drawable.texture;
        pass.colorAttachments[0].loadAction = MTLLoadActionClear;
        pass.colorAttachments[0].storeAction = MTLStoreActionStore;
        pass.colorAttachments[0].clearColor = MTLClearColorMake(0, 0, 0, 1);
        id<MTLCommandBuffer> command = [queue_ commandBuffer];
        id<MTLRenderCommandEncoder> render = [command renderCommandEncoderWithDescriptor:pass];
        if (!command || !render) throw std::runtime_error("Metal render command unavailable");
        [render setRenderPipelineState:pipeline_]; [render setViewport:viewport];
        [render setVertexBytes:&rotation_ length:sizeof(rotation_) atIndex:0];
        const float visible_fraction[2] = {float(width_)/geometry_.coded_width, float(height_)/geometry_.coded_height};
        [render setVertexBytes:visible_fraction length:sizeof(visible_fraction) atIndex:1];
        [render setFragmentTexture:CVMetalTextureGetTexture(lease->y.get()) atIndex:0];
        [render setFragmentTexture:CVMetalTextureGetTexture(lease->uv.get()) atIndex:1];
        const auto cursor = cursor_.parameters(viewport,rotated_w,rotated_h);
        [render setFragmentTexture:cursor_.image(device_) atIndex:2];
        [render setFragmentBytes:&cursor length:sizeof(cursor) atIndex:0];
        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4];
        [render endEncoding];
        id<MTLBuffer> samples = nil;
        if (verify) {
            samples = [device_ newBufferWithLength:4*256 options:MTLResourceStorageModeShared];
            id<MTLBlitCommandEncoder> blit = [command blitCommandEncoder];
            if (!samples || !blit) throw std::runtime_error("Diagnostic sample buffer unavailable");
            for (unsigned q = 0; q < 4; ++q) {
                const NSUInteger x = std::min(drawable.texture.width-1, NSUInteger(viewport.originX + viewport.width*(q%2 ? 0.75 : 0.25)));
                const NSUInteger y = std::min(drawable.texture.height-1, NSUInteger(viewport.originY + viewport.height*(q/2 ? 0.75 : 0.25)));
                [blit copyFromTexture:drawable.texture sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(x,y,0)
                    sourceSize:MTLSizeMake(1,1,1) toBuffer:samples destinationOffset:q*256 destinationBytesPerRow:256 destinationBytesPerImage:256];
            }
            [blit endEncoding];
        }
        const auto stats = stats_;
        const auto submission = ++submission_;
        [drawable addPresentedHandler:^(id<MTLDrawable> shown) {
            // Command completion/decoded output are not a physical display.
            // Only distinct, positively timestamped presentations count.
            std::lock_guard<std::mutex> lock(stats->mutex);
            ++stats->callbacks;
            // Resize can redraw the same video index. Order drawable results
            // separately so an older callback cannot clear a newer retry.
            if (submission >= stats->result_submission) {
                stats->result_submission = submission;
                if (shown.presentedTime <= 0) stats->dropped = index;
                else stats->dropped.reset();
            }
            if (shown.presentedTime <= 0) {
                ++stats->zero_timestamps;
                return;
            }
            stats->confirmed_submission = std::max(stats->confirmed_submission, submission);
            if (!stats->seen || index > stats->last_index) {
                ++stats->displayed; stats->last_index = index; stats->seen = true;
            }
        }];
        [command addCompletedHandler:^(id<MTLCommandBuffer>) { (void)lease; }];
        [command presentDrawable:drawable]; [command commit];
        const auto waiting = Clock::now(); [command waitUntilCompleted];
        gpu_wait_us += std::chrono::duration<double,std::micro>(Clock::now()-waiting).count(); ++draw_count;
        if (command.status != MTLCommandBufferStatusCompleted) throw std::runtime_error("Metal frame submission failed");
        if (verify) {
            const auto* bytes = static_cast<const uint8_t*>(samples.contents);
            for (unsigned q = 0; q < 4; ++q) {
                unsigned x = q%2, y = q/2;
                if (rotation_ == 1) { const auto old = x; x = y; y = 1-old; }
                else if (rotation_ == 2) { x = 1-x; y = 1-y; }
                else if (rotation_ == 3) { const auto old = x; x = 1-y; y = old; }
                const auto expected = test_rgb(index, y*2+x);
                const auto* pixel = bytes + q*256;
                const unsigned error = std::max({unsigned(std::abs(int(pixel[2])-int(expected[0]))),
                    unsigned(std::abs(int(pixel[1])-int(expected[1]))), unsigned(std::abs(int(pixel[0])-int(expected[2])))});
                max_color_error = std::max(max_color_error, error); ++pixel_checks;
                if (error > 12) throw std::runtime_error("Metal quadrant mismatch at frame " + std::to_string(index) +
                    " quadrant " + std::to_string(q) + " error " + std::to_string(error));
            }
        }
        return true;
    }
public:
    uint64_t pixel_checks = 0;
    uint32_t max_color_error = 0;
    double drawable_us = 0, gpu_wait_us = 0;
    uint64_t draw_count = 0;
    NativePresenter(unsigned width, unsigned height, unsigned rotation, bool verify, NSView* parent = nil)
        : width_(width), height_(height), rotation_(rotation), geometry_(width,height), verify_(verify) {
        require_main();
        if (rotation > 3) throw std::runtime_error("Invalid view geometry");
        device_ = MTLCreateSystemDefaultDevice();
        queue_ = [device_ newCommandQueue];
        if (!device_ || !queue_) throw std::runtime_error("Metal device unavailable");
        NSString* source = video_shader();
        NSError* error = nil;
        id<MTLLibrary> library = [device_ newLibraryWithSource:source options:nil error:&error];
        if (!library) throw std::runtime_error("Metal shader compilation failed: " + std::string(error.localizedDescription.UTF8String ?: "unknown"));
        MTLRenderPipelineDescriptor* pipeline = [[MTLRenderPipelineDescriptor alloc] init];
        pipeline.vertexFunction = [library newFunctionWithName:@"video_vertex"];
        pipeline.fragmentFunction = [library newFunctionWithName:@"video_fragment"];
        pipeline.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
        pipeline_ = [device_ newRenderPipelineStateWithDescriptor:pipeline error:&error];
        if (!pipeline_) throw std::runtime_error("Metal pipeline unavailable");
        CVMetalTextureCacheRef cache = nullptr;
        check(CVMetalTextureCacheCreate(kCFAllocatorDefault, nullptr, device_, nullptr, &cache), "Metal texture cache");
        cache_ = OwnedCF<CVMetalTextureCacheRef>(cache);
        if (!parent) {
            [NSApplication sharedApplication];
            if (![NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory]) throw std::runtime_error("Native viewer activation unavailable");
            [NSApp finishLaunching];
            window_ = [[NSWindow alloc] initWithContentRect:NSMakeRect(120,120,960,540)
                styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskMiniaturizable|NSWindowStyleMaskResizable
                backing:NSBackingStoreBuffered defer:NO];
            window_.releasedWhenClosed = NO; window_.title = @"Pixels remote desktop — native media check";
            parent = window_.contentView; owns_window_ = true;
        } else window_ = parent.window;
        auto frame = parent.bounds;
        if (!owns_window_) {
            // Full-size content views include the titlebar. Match the WebView's
            // unobscured content area before reserving its shared titlebar.
            frame = [parent convertRect:window_.contentLayoutRect fromView:nil];
            frame.size.height = std::max(0.0,frame.size.height-42);
            if (parent.flipped) frame.origin.y += 42;
        }
        view_ = [[PABMetalView alloc] initWithFrame:frame];
        view_->mediaInput.width = width; view_->mediaInput.height = height; view_->mediaInput.rotation = rotation;
        if (owns_window_) view_.autoresizingMask = NSViewWidthSizable|NSViewHeightSizable;
        else view_.translatesAutoresizingMaskIntoConstraints = NO;
        view_.wantsLayer = YES;
        CAMetalLayer* layer = (CAMetalLayer*)view_.layer;
        layer.device = device_; layer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        layer.framebufferOnly = !verify_; layer.displaySyncEnabled = YES; layer.maximumDrawableCount = 3;
        OwnedCF<CGColorSpaceRef> color(CGColorSpaceCreateWithName(kCGColorSpaceITUR_709));
        layer.colorspace = color.get();
        [parent addSubview:view_];
        if (!owns_window_) {
            // The guide follows titlebar/fullscreen and window size changes;
            // autoresizing alone keeps a stale top inset after those changes.
            NSLayoutGuide* layout = window_.contentLayoutGuide;
            [NSLayoutConstraint activateConstraints:@[
                [view_.leadingAnchor constraintEqualToAnchor:layout.leadingAnchor],
                [view_.trailingAnchor constraintEqualToAnchor:layout.trailingAnchor],
                [view_.topAnchor constraintEqualToAnchor:layout.topAnchor constant:42],
                [view_.bottomAnchor constraintEqualToAnchor:layout.bottomAnchor]
            ]];
            [parent layoutSubtreeIfNeeded];
        }
        [view_ setNeedsLayout:YES];
        if (owns_window_) {
            [window_ makeKeyAndOrderFront:nil]; [NSApp activateIgnoringOtherApps:YES];
            const auto deadline = Clock::now() + std::chrono::seconds(2);
            while (!(window_.occlusionState & NSWindowOcclusionStateVisible) && Clock::now() < deadline) {
                pump_events(); pump();
            }
        }
    }
    ~NativePresenter() {
        view_->mediaInput.release(); view_->mediaInput.enabled = false; view_->mediaInput.sink = {};
        [view_ removeFromSuperview];
        if (owns_window_) { [window_ orderOut:nil]; [window_ close]; }
    }
    void pump_events() {
        require_main();
        @autoreleasepool {
            if (owns_window_) {
                for (unsigned count = 0; count < 64; ++count) {
                    NSEvent* event = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:NSDate.distantPast inMode:NSDefaultRunLoopMode dequeue:YES];
                    if (!event) break; [NSApp sendEvent:event];
                }
                [NSApp updateWindows];
            }
            if (!view_.window) throw std::runtime_error("Native view detached");
            if (owns_window_ && !window_.visible && !window_.miniaturized) throw std::runtime_error("Native viewer closed");
            bool dropped = false;
            {
                std::lock_guard<std::mutex> lock(stats_->mutex);
                dropped = stats_->dropped && *stats_->dropped == last_index_;
                if (dropped && Clock::now() >= next_redraw_) stats_->dropped.reset();
            }
            if ((view_.needsMediaRedraw || dropped) && last_image_.get() && Clock::now() >= next_redraw_) {
                next_redraw_ = Clock::now() + std::chrono::milliseconds(50);
                if (draw(last_image_.get(), last_index_, verify_ && !last_verified_)) last_verified_ = true;
            }
        }
    }
    void render(CVPixelBufferRef image, uint64_t index) {
        last_image_ = OwnedCF<CVPixelBufferRef>(CVPixelBufferRetain(image)); last_index_ = index;
        last_verified_ = draw(image, index, verify_);
    }
    void input_sink(std::function<bool(const InputPacket&)> sink) { require_main(); view_->mediaInput.sink = std::move(sink); }
    void cursor(const uint8_t* bytes, size_t size) {
        if (size && bytes[0] == 2) {
            require_main();
            if (size > 2049) throw std::runtime_error("Viewer statistics too large");
            NSString* text = [[NSString alloc] initWithBytes:bytes+1 length:size-1 encoding:NSUTF8StringEncoding];
            if (!text) throw std::runtime_error("Invalid viewer statistics text");
            if (!view_.statisticsOverlay && text.length) {
                view_.statisticsOverlay = [[PABViewerStats alloc] initWithFrame:NSZeroRect];
                [view_ addSubview:view_.statisticsOverlay positioned:NSWindowAbove relativeTo:nil];
            }
            view_.statisticsOverlay.statisticsText = text;
            view_.statisticsOverlay.hidden = !text.length;
            [view_.statisticsOverlay setAccessibilityLabel:[text componentsSeparatedByString:@"\n"].firstObject];
            [view_.statisticsOverlay placeInViewer]; view_.statisticsOverlay.needsDisplay = YES;
            return;
        }
        require_main(); cursor_.update(device_,bytes,size);
        if (view_.tracksRemoteCursor != cursor_.tracking()) {
            view_.tracksRemoteCursor = cursor_.tracking();
            [view_.window invalidateCursorRectsForView:view_];
        }
        view_.needsMediaRedraw = YES;
    }
    void input_enabled(bool value) { require_main(); [view_ setMediaInputEnabled:value]; }
    uint64_t displayed() { std::lock_guard<std::mutex> lock(stats_->mutex); return stats_->displayed; }
    void finish() {
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        while (last_image_.get() && Clock::now() < deadline) {
            { std::lock_guard<std::mutex> lock(stats_->mutex);
                if (stats_->seen && stats_->last_index == last_index_ && stats_->confirmed_submission == submission_) return; }
            pump_events(); pump();
        }
    }
    // Explicit generated-frame diagnostic; never available for embedded views.
    // Exercise the same resize/redraw/minimize/close paths without sending
    // system input or touching another application's windows.
    void verify_lifecycle() {
        require_main();
        if (!owns_window_ || !verify_ || !last_image_.get())
            throw std::runtime_error("Lifecycle check requires a generated standalone frame");
        finish();
        const auto count = displayed();
        if (!count) {
            std::lock_guard<std::mutex> lock(stats_->mutex);
            throw std::runtime_error("Lifecycle check has no displayed frame: draws=" + std::to_string(draw_count) +
                " callbacks=" + std::to_string(stats_->callbacks) + " zero_timestamps=" + std::to_string(stats_->zero_timestamps));
        }
        const auto settle = [&] {
            const auto deadline = Clock::now() + std::chrono::milliseconds(150);
            while (Clock::now() < deadline) { pump_events(); pump(); }
        };
        const auto confirm = [&] {
            finish();
            std::lock_guard<std::mutex> lock(stats_->mutex);
            if (stats_->confirmed_submission != submission_) throw std::runtime_error("Resized frame was not displayed");
        };
        for (const auto size : {NSMakeSize(640,960), NSMakeSize(1050,450)}) {
            [window_ setContentSize:size]; [view_ setNeedsLayout:YES];
            settle(); draw(last_image_.get(), last_index_, true); confirm();
            if (displayed() != count) throw std::runtime_error("Static redraw invented a new frame");
        }
        [window_ miniaturize:nil]; settle();
        if (!window_.miniaturized) throw std::runtime_error("Native test window did not minimize");
        const auto before = draw_count;
        draw(last_image_.get(), last_index_, true);
        if (draw_count != before) throw std::runtime_error("Minimized window submitted GPU work");
        [window_ deminiaturize:nil]; [window_ makeKeyAndOrderFront:nil];
        view_.needsMediaRedraw = YES; settle();
        if (window_.miniaturized || !window_.visible) throw std::runtime_error("Native test window did not restore");
        draw(last_image_.get(), last_index_, true); settle(); confirm();
        if (displayed() != count) throw std::runtime_error("Restored static frame counted twice");
        [window_ close];
        bool closed = false;
        try { pump_events(); }
        catch (const std::runtime_error& error) { closed = std::string(error.what()) == "Native viewer closed"; if (!closed) throw; }
        if (!closed) throw std::runtime_error("Closed native view did not terminate its loop");
        fprintf(stderr, "media lifecycle=resize_minimize_restore_close_passed static_display_count=%llu\n", count);
    }
};
}
