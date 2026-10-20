// Run on a Mac. Generated shape tests require no screen recording or input.
#include "../native/macos/cursor_source.h"
#include <cassert>
#include <limits>
using namespace pab_mac;
// A bounded, self-owned foreground window for live IPC checks. Commands change
// only its cursor; the former foreground app and original pointer are restored
// on normal completion. An external test guard also bounds the process lifetime.
static void fixture() {
    [NSApplication sharedApplication];
    NSRunningApplication* previous=NSWorkspace.sharedWorkspace.frontmostApplication;
    OwnedCF<CGEventRef> original(CGEventCreate(nullptr));
    const CGPoint position=CGEventGetLocation(original.get());
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [NSApp finishLaunching];
    NSWindow* window=[[NSWindow alloc] initWithContentRect:NSMakeRect(250,250,480,240)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    window.title=@"Pixels cursor source acceptance";
    [window makeKeyAndOrderFront:nil]; [NSApp activateIgnoringOtherApps:YES];
    const CGRect screen=CGDisplayBounds(CGMainDisplayID());
    const CGPoint target=CGPointMake(450,screen.size.height-350);
    check(CGWarpMouseCursorPosition(target),"Fixture pointer move");
    NSBitmapImageRep* rep=[[NSBitmapImageRep alloc] initWithBitmapDataPlanes:nullptr pixelsWide:9 pixelsHigh:7
        bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSCalibratedRGBColorSpace
        bitmapFormat:NSBitmapFormatAlphaNonpremultiplied bytesPerRow:36 bitsPerPixel:32];
    for (int y=0;y<7;++y) for (int x=0;x<9;++x)
        [rep setColor:[NSColor colorWithCalibratedRed:x/8.0 green:y/6.0 blue:0.25 alpha:0.5] atX:x y:y];
    NSImage* image=[[NSImage alloc] initWithSize:NSMakeSize(9,7)];[image addRepresentation:rep];
    NSCursor* custom=[[NSCursor alloc] initWithImage:image hotSpot:NSMakePoint(3,2)];
    NSCursor* selected=NSCursor.arrowCursor;
    bool hidden=false;
    @try {
        write_value(uint32_t(0x31465843));
        for (;;) {
            const uint8_t command=read_value<uint8_t>([&]{
                for (unsigned n=0;n<32;++n) {
                    NSEvent* event=[NSApp nextEventMatchingMask:NSEventMaskAny untilDate:NSDate.distantPast inMode:NSDefaultRunLoopMode dequeue:YES];
                    if(!event)break; [NSApp sendEvent:event];
                }
                if(!hidden)[selected set];
            });
            if (!command) break;
            if (command>6) throw std::runtime_error("Invalid fixture command");
            if(hidden) { [NSCursor unhide]; hidden=false; }
            selected=(command==2 || command==3 || command>=5) ? custom : NSCursor.arrowCursor;
            if(command>=5) check(CGWarpMouseCursorPosition(CGPointMake(target.x+(command==5?20:0),target.y)),"Fixture pointer move");
            [selected set];
            if(command==3) { [NSCursor hide]; hidden=true; }
            const auto deadline=Clock::now()+std::chrono::milliseconds(100);
            while(Clock::now()<deadline) { if(!hidden)[selected set]; pump(); }
            write_value(command);
        }
    } @finally {
        if(hidden)[NSCursor unhide];
        [NSCursor.arrowCursor set]; [window orderOut:nil];
        OwnedCF<CGEventRef> now(CGEventCreate(nullptr));
        if(now.get() && (CGPointEqualToPoint(CGEventGetLocation(now.get()),target) ||
            CGPointEqualToPoint(CGEventGetLocation(now.get()),CGPointMake(target.x+20,target.y)))) CGWarpMouseCursorPosition(position);
        if(NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier==getpid())
            [previous activateWithOptions:NSApplicationActivateIgnoringOtherApps];
    }
}
static void shape_checks() {
    NSBitmapImageRep* rep = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:nullptr pixelsWide:4 pixelsHigh:3
        bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSCalibratedRGBColorSpace
        bitmapFormat:NSBitmapFormatAlphaNonpremultiplied bytesPerRow:16 bitsPerPixel:32];
    for (unsigned y=0;y<3;++y) for (unsigned x=0;x<4;++x) {
        [rep setColor:[NSColor colorWithCalibratedRed:(x+1)/5.0 green:(y+1)/4.0 blue:0.2 alpha:0.5] atX:x y:y];
    }
    NSImage* image = [[NSImage alloc] initWithSize:NSMakeSize(4,3)]; [image addRepresentation:rep];
    auto base = cursor_shape(image,NSMakePoint(1,2),1,1);
    assert(base.width==4 && base.height==3 && base.hotspot_x==1 && base.hotspot_y==2);
    // Independent color reference checks BGRA order, top-down orientation,
    // premultiplication, and asymmetric x/y content. sRGB input below avoids
    // a test depending on the monitor's color profile.
    unsigned samples=0;
    for (unsigned y=0;y<3;++y) for (unsigned x=0;x<4;++x) {
        NSColor* c=[[rep colorAtX:x y:y] colorUsingColorSpace:NSColorSpace.sRGBColorSpace];
        const double channels[]={c.blueComponent,c.greenComponent,c.redComponent,1};
        for (unsigned q=0;q<4;++q) {
            const int expected=std::lround(channels[q]*c.alphaComponent*255);
            assert(std::abs(int(base.pixels[(y*4+x)*4+q])-expected)<=2); ++samples;
        }
    }
    auto doubled=cursor_shape(image,NSMakePoint(1,2),2,2);
    assert(doubled.width==8 && doubled.height==6 && doubled.hotspot_x==2 && doubled.hotspot_y==4);
    // Representation resolution is not the on-screen size.
    image.size=NSMakeSize(2,1.5);
    auto small=cursor_shape(image,NSMakePoint(0,0),1,1);
    assert(small.width==2 && small.height==2);
    unsigned rejected=0;
    for (const NSPoint point : {NSMakePoint(-1,0),NSMakePoint(512,0),NSMakePoint(0,NAN)}) {
        try { (void)cursor_shape(image,point,1,1); } catch(const std::exception&) { ++rejected; }
    }
    for (double scale : {0.0,-1.0,double(NAN),double(INFINITY),1024.0}) {
        try { (void)cursor_shape(image,NSZeroPoint,scale,scale); } catch(const std::exception&) { ++rejected; }
    }
    assert(rejected==8);
    printf("{\"color_channels_checked\":%u,\"invalid_geometry_rejected\":%u,\"scale_checks\":3}\n",samples,rejected);
}
int main(int argc,const char* argv[]) { @autoreleasepool {
    try {
        if (argc==1) { shape_checks(); return 0; }
        if (argc==2 && std::string(argv[1])=="fixture") { fixture(); return 0; }
        if (argc==2 && std::string(argv[1])=="inventory") { inventory(displays()); return 0; }
        if (argc==3 && std::string(argv[1])=="cursor") {
            for (const auto& d : displays()) if (d.id==number(argv[2],UINT32_MAX)) { cursor_worker(d);return 0; }
        }
        throw std::runtime_error("Unknown test operation");
    } catch(const std::exception& e) { fprintf(stderr,"%s\n",e.what());return 1; }
} }
