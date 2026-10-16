// Local AppKit event-dispatch check. Never posts system input or reads a screen.
// Compile on macOS with -I crates/desktop-media/native/macos, -fobjc-arc,
// -fblocks, -std=c++17 and Foundation/AppKit/ApplicationServices frameworks.
#include "viewer_input.h"
#include <iostream>

static unsigned checks = 0;
static void expect(bool condition, const char* label) {
    if (!condition) throw std::runtime_error(label);
    ++checks;
}
static uint16_t field(const pab_mac::InputPacket& packet, unsigned offset) {
    return uint16_t(packet[offset]) | uint16_t(packet[offset+1]) << 8;
}
static NSEvent* mouse(PABInputView* view, NSEventType type, double x, double y) {
    const auto point = [view convertPoint:NSMakePoint(x,y) toView:nil];
    return [NSEvent mouseEventWithType:type location:point modifierFlags:0 timestamp:0
        windowNumber:view.window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:0];
}
static NSEvent* key(PABInputView* view, NSEventType type, unsigned code, NSUInteger flags = 0) {
    return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:flags timestamp:0
        windowNumber:view.window.windowNumber context:nil characters:@"" charactersIgnoringModifiers:@""
        isARepeat:NO keyCode:code];
}
int main() {
    @autoreleasepool {
        try {
            [NSApplication sharedApplication];
            [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
            [NSApp finishLaunching];
            NSWindow* window = [[NSWindow alloc] initWithContentRect:NSMakeRect(80,80,800,600)
                styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
            window.releasedWhenClosed = NO;
            PABInputView* view = [[PABInputView alloc] initWithFrame:window.contentView.bounds];
            [window.contentView addSubview:view];
            std::vector<pab_mac::InputPacket> packets;
            view->mediaInput.width = 1920; view->mediaInput.height = 1080;
            view->mediaInput.sink = [&](const auto& packet) { packets.push_back(packet); return true; };
            [view mouseDown:mouse(view,NSEventTypeLeftMouseDown,400,300)];
            expect(packets.empty(), "view-only sent input");
            [view setMediaInputEnabled:YES];
            expect(packets.back()[0]==6 && field(packets.back(),5)==1, "enable barrier missing");
            for (unsigned rotation = 0; rotation < 4; ++rotation) {
                view->mediaInput.rotation = rotation;
                packets.clear();
                [view mouseMoved:mouse(view,NSEventTypeMouseMoved,400,300)];
                expect(packets.size()==1 && packets[0][0]==1, "center motion missing");
                expect(std::abs(int(field(packets[0],1))-32768)<200 &&
                    std::abs(int(field(packets[0],3))-32768)<200, "center mapping wrong");
                // Check opposite image corners in backing pixels, including
                // the letterbox offset and AppKit's bottom-up coordinates.
                const auto backing = [view convertRectToBacking:view.bounds];
                const double w = rotation%2 ? 1080 : 1920, h = rotation%2 ? 1920 : 1080;
                const double scale = std::min(backing.size.width/w,backing.size.height/h);
                const double left = (backing.size.width-w*scale)/2;
                const double bottom = (backing.size.height-h*scale)/2;
                for (unsigned corner=0; corner<2; ++corner) {
                    const auto point = [view convertPointFromBacking:NSMakePoint(
                        left + (corner ? w*scale-1 : 0), bottom + (corner ? 1 : h*scale))];
                    packets.clear();
                    [view mouseMoved:mouse(view,NSEventTypeMouseMoved,point.x,point.y)];
                    expect(packets.size()==1 && field(packets[0],1)==(corner ? 65535 : 0) &&
                        field(packets[0],3)==(corner ? 65535 : 0), "image corner mapping wrong");
                }
                packets.clear();
                [view mouseDown:mouse(view,NSEventTypeLeftMouseDown,1,1)];
                expect(packets.empty(), "black border sent click");
                [view mouseDown:mouse(view,NSEventTypeLeftMouseDown,400,300)];
                [view mouseDragged:mouse(view,NSEventTypeLeftMouseDragged,-50,-50)];
                [view mouseUp:mouse(view,NSEventTypeLeftMouseUp,-50,-50)];
                expect(packets.size()==3 && packets.back()[0]==2 && !packets.back()[7], "outside drag lost release");
                expect(field(packets.back(),1)==0 && field(packets.back(),3)==65535 && !view->mediaInput.buttons,
                    "outside drag not clamped to image edge");
            }
            packets.clear();
            [view keyDown:key(view,NSEventTypeKeyDown,kVK_ANSI_A)];
            [view keyUp:key(view,NSEventTypeKeyUp,kVK_ANSI_A)];
            expect(packets.size()==2 && field(packets[0],5)==65 && packets[0][7] && !packets[1][7], "physical key mapping failed");
            packets.clear();
            [view flagsChanged:key(view,NSEventTypeFlagsChanged,kVK_Shift,NX_DEVICELSHIFTKEYMASK|NX_DEVICERSHIFTKEYMASK)];
            [view flagsChanged:key(view,NSEventTypeFlagsChanged,kVK_RightShift,NX_DEVICELSHIFTKEYMASK|NX_DEVICERSHIFTKEYMASK)];
            [view flagsChanged:key(view,NSEventTypeFlagsChanged,kVK_Shift,NX_DEVICERSHIFTKEYMASK)];
            [view flagsChanged:key(view,NSEventTypeFlagsChanged,kVK_RightShift,0)];
            expect(packets.size()==4 && field(packets[0],5)==160 && field(packets[1],5)==161 &&
                packets[0][7] && packets[1][7] && !packets[2][7] && !packets[3][7], "left/right modifier release failed");
            packets.clear();
            [view resignFirstResponder];
            expect(packets.size()==1 && packets[0][0]==5, "focus loss did not release input");
            packets.clear();
            [view setMediaInputEnabled:NO];
            expect(packets.size()>=2 && packets[0][0]==5 && packets[1][0]==6 && !field(packets[1],5), "disable barrier failed");
            const auto disabled = packets.size();
            [view keyDown:key(view,NSEventTypeKeyDown,kVK_ANSI_A)];
            [view mouseDown:mouse(view,NSEventTypeLeftMouseDown,400,300)];
            expect(packets.size()==disabled, "disabled view sent input");
            view->mediaInput.sink = [](const auto&) { return false; };
            [view setMediaInputEnabled:YES];
            expect(!view->mediaInput.enabled, "sink failure left input enabled");
            view->mediaInput.sink = {};
            [view removeFromSuperview]; [window close];
            std::cout << "{\"checks\":" << checks << ",\"local_events_only\":true,\"system_input_posted\":false}" << std::endl;
            return 0;
        } catch (const std::exception& error) { std::cerr << error.what() << std::endl; return 1; }
    }
}
