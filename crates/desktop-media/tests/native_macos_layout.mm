// Opt-in AppKit regression for the product toolbar; generated views only.
// Compile with -I native/macos and the same frameworks as the native presenter.
#include <presenter.h>

static void require(bool value, const char* message) {
    if (!value) throw std::runtime_error(message);
}
static void verify(NSWindow* window, NSView* parent) {
    [parent layoutSubtreeIfNeeded];
    PABMetalView* media = nil;
    for(NSView* child in parent.subviews)
        if([child isKindOfClass:PABMetalView.class]) media=(PABMetalView*)child;
    require(media!=nil,"Native media view missing");
    const NSRect usable=[parent convertRect:window.contentLayoutRect fromView:nil];
    const NSRect frame=media.frame;
    require(fabs(NSMaxY(frame)-(NSMaxY(usable)-72))<.5,"Native media overlaps toolbar/titlebar");
    require(fabs(NSMinY(frame)-NSMinY(usable))<.5,"Native media bottom differs from usable content");
    require(fabs(frame.size.width-usable.size.width)<.5,"Native media width differs from usable content");
    // A point in the 72-point web toolbar must never hit the Metal input view.
    NSView* hit=[parent hitTest:NSMakePoint(NSMidX(usable),NSMaxY(usable)-50)];
    require(hit!=media,"Native input view intercepts toolbar clicks");
}
int main() {
    @autoreleasepool {
        try {
            [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];[NSApp finishLaunching];
            unsigned checks=0;
            for(bool full : {false,true}) {
                NSWindowStyleMask style=NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskResizable;
                if(full) style|=NSWindowStyleMaskFullSizeContentView;
                NSWindow* window=[[NSWindow alloc] initWithContentRect:NSMakeRect(100,100,960,600) styleMask:style backing:NSBackingStoreBuffered defer:NO];
                window.releasedWhenClosed=NO;
                [window orderFrontRegardless];
                NSView* parent=window.contentView;
                {
                    pab_mac::NativePresenter presenter(1920,1080,0,false,parent);
                    for(NSSize size : {NSMakeSize(960,600),NSMakeSize(640,400),NSMakeSize(1200,800)}) {
                        [window setContentSize:size];verify(window,parent);++checks;
                    }
                    window.styleMask ^= NSWindowStyleMaskFullSizeContentView;
                    verify(window,parent);++checks;
                }
                for(NSView* child in parent.subviews)require(![child isKindOfClass:PABMetalView.class],"Native media view leaked after close");
                [window orderOut:nil];[window close];
            }
            printf("{\"layout_cases\":%u,\"toolbar_hit_test\":true,\"native_view_cleanup\":true}\n",checks);
            return 0;
        } catch(const std::exception& error) {
            fprintf(stderr,"layout regression: %s\n",error.what());return 1;
        }
    }
}
