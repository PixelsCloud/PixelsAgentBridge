#pragma once
#include "common.h"
#include <array>
#import <Carbon/Carbon.h>
#import <IOKit/hidsystem/IOLLEvent.h>

namespace pab_mac {
using InputPacket = std::array<uint8_t, 8>;
struct ViewInput {
    std::function<bool(const InputPacket&)> sink;
    bool enabled = false;
    bool fullscreen = false, escape_held = false;
    unsigned buttons = 0, width = 0, height = 0, rotation = 0;
    double wheel = 0;
    void emit(uint8_t kind, uint16_t x = 0, uint16_t y = 0, uint16_t value = 0, bool down = false) {
        const InputPacket packet{kind,uint8_t(x),uint8_t(x>>8),uint8_t(y),uint8_t(y>>8),uint8_t(value),uint8_t(value>>8),uint8_t(down)};
        if (!sink || !sink(packet)) { enabled = false; buttons = 0; wheel = 0; }
    }
    void release() { if (enabled) emit(5); buttons = 0; wheel = 0; }
    void enable(bool value) { release(); enabled = value && bool(sink); emit(6,0,0,enabled ? 1 : 0); }
    bool position(NSView* view, NSEvent* event, uint16_t& x, uint16_t& y) const {
        if (!width || !height) return false;
        const double w = rotation%2 ? height : width, h = rotation%2 ? width : height;
        // Use the same backing-pixel geometry as Metal, including Retina.
        const auto bounds = [view convertRectToBacking:view.bounds];
        const double scale = std::min(bounds.size.width/w,bounds.size.height/h);
        if (scale <= 0) return false;
        const auto point = [view convertPointToBacking:[view convertPoint:event.locationInWindow fromView:nil]];
        const double left = (bounds.size.width-w*scale)/2, top = (bounds.size.height-h*scale)/2;
        const double px = point.x-bounds.origin.x-left;
        const double py = (view.flipped ? point.y-bounds.origin.y : NSMaxY(bounds)-point.y)-top;
        if (!buttons && (px < 0 || py < 0 || px >= w*scale || py >= h*scale)) return false;
        const auto normalized = [](double value, double length) {
            return uint16_t(std::lround(std::clamp(value/std::max(1.0,length-1),0.0,1.0)*65535));
        };
        x = normalized(px,w*scale); y = normalized(py,h*scale); return true;
    }
};
inline uint16_t windows_key(unsigned code) {
    // Public macOS physical key codes -> the existing platform-neutral VK
    // contract. Text composition/IME needs a separate protocol capability.
    static const std::pair<unsigned,uint16_t> keys[] = {
        {kVK_ANSI_A,65},{kVK_ANSI_B,66},{kVK_ANSI_C,67},{kVK_ANSI_D,68},{kVK_ANSI_E,69},{kVK_ANSI_F,70},
        {kVK_ANSI_G,71},{kVK_ANSI_H,72},{kVK_ANSI_I,73},{kVK_ANSI_J,74},{kVK_ANSI_K,75},{kVK_ANSI_L,76},
        {kVK_ANSI_M,77},{kVK_ANSI_N,78},{kVK_ANSI_O,79},{kVK_ANSI_P,80},{kVK_ANSI_Q,81},{kVK_ANSI_R,82},
        {kVK_ANSI_S,83},{kVK_ANSI_T,84},{kVK_ANSI_U,85},{kVK_ANSI_V,86},{kVK_ANSI_W,87},{kVK_ANSI_X,88},
        {kVK_ANSI_Y,89},{kVK_ANSI_Z,90},
        {kVK_ANSI_0,48},{kVK_ANSI_1,49},{kVK_ANSI_2,50},{kVK_ANSI_3,51},{kVK_ANSI_4,52},
        {kVK_ANSI_5,53},{kVK_ANSI_6,54},{kVK_ANSI_7,55},{kVK_ANSI_8,56},{kVK_ANSI_9,57},
        {kVK_Return,13},{kVK_Tab,9},{kVK_Space,32},{kVK_Delete,8},{kVK_ForwardDelete,46},{kVK_Escape,27},
        {kVK_LeftArrow,37},{kVK_UpArrow,38},{kVK_RightArrow,39},{kVK_DownArrow,40},
        {kVK_Home,36},{kVK_End,35},{kVK_PageUp,33},{kVK_PageDown,34},{kVK_Help,45},
        {kVK_ANSI_Semicolon,186},{kVK_ANSI_Equal,187},{kVK_ANSI_Comma,188},{kVK_ANSI_Minus,189},
        {kVK_ANSI_Period,190},{kVK_ANSI_Slash,191},{kVK_ANSI_Grave,192},{kVK_ANSI_LeftBracket,219},
        {kVK_ANSI_Backslash,220},{kVK_ANSI_RightBracket,221},{kVK_ANSI_Quote,222},
        {kVK_F1,112},{kVK_F2,113},{kVK_F3,114},{kVK_F4,115},{kVK_F5,116},{kVK_F6,117},
        {kVK_F7,118},{kVK_F8,119},{kVK_F9,120},{kVK_F10,121},{kVK_F11,122},{kVK_F12,123},
        {kVK_ANSI_Keypad0,96},{kVK_ANSI_Keypad1,97},{kVK_ANSI_Keypad2,98},{kVK_ANSI_Keypad3,99},
        {kVK_ANSI_Keypad4,100},{kVK_ANSI_Keypad5,101},{kVK_ANSI_Keypad6,102},{kVK_ANSI_Keypad7,103},
        {kVK_ANSI_Keypad8,104},{kVK_ANSI_Keypad9,105},{kVK_ANSI_KeypadMultiply,106},
        {kVK_ANSI_KeypadPlus,107},{kVK_ANSI_KeypadMinus,109},{kVK_ANSI_KeypadDecimal,110},
        {kVK_ANSI_KeypadDivide,111},{kVK_ANSI_KeypadEnter,13},{kVK_ANSI_KeypadClear,144}
    };
    for (const auto key : keys) if (key.first == code) return key.second;
    return 0;
}
}

// Events belong only to this focused view; no global hook/accessibility grant.
@interface PABInputView : NSView {
@public
    pab_mac::ViewInput mediaInput;
    NSTrackingArea* __strong mediaTracking;
}
- (void)setMediaInputEnabled:(BOOL)enabled;
@end
@implementation PABInputView
- (BOOL)acceptsFirstResponder { return mediaInput.enabled || mediaInput.fullscreen; }
- (BOOL)acceptsFirstMouse:(NSEvent*)event { (void)event; return YES; }
- (BOOL)resignFirstResponder { mediaInput.release(); return [super resignFirstResponder]; }
- (void)setMediaInputEnabled:(BOOL)enabled {
    mediaInput.enable(enabled);
    if (mediaInput.enabled || mediaInput.fullscreen) [self.window makeFirstResponder:self];
    else if (self.window.firstResponder == self) [self.window makeFirstResponder:nil];
}
- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (mediaTracking) [self removeTrackingArea:mediaTracking];
    mediaTracking = [[NSTrackingArea alloc] initWithRect:NSZeroRect
        options:NSTrackingMouseMoved|NSTrackingActiveInKeyWindow|NSTrackingInVisibleRect owner:self userInfo:nil];
    [self addTrackingArea:mediaTracking];
}
- (void)keyDown:(NSEvent*)event {
    if (event.keyCode == kVK_Escape && (mediaInput.fullscreen || mediaInput.escape_held)) {
        if (!mediaInput.escape_held) { mediaInput.escape_held = true; mediaInput.release(); mediaInput.emit(8); }
        return;
    }
    if (!mediaInput.enabled) return;
    const auto key = pab_mac::windows_key(event.keyCode);
    if (key) mediaInput.emit(4,0,0,key,true);
}
- (void)keyUp:(NSEvent*)event {
    if (event.keyCode == kVK_Escape && mediaInput.escape_held) { mediaInput.escape_held = false; return; }
    if (!mediaInput.enabled) return;
    const auto key = pab_mac::windows_key(event.keyCode);
    if (key) mediaInput.emit(4,0,0,key,false);
}
- (BOOL)performKeyEquivalent:(NSEvent*)event {
    if ((!mediaInput.enabled && !mediaInput.fullscreen) || self.window.firstResponder != self) return NO;
    [self keyDown:event]; return YES;
}
- (void)flagsChanged:(NSEvent*)event {
    if (!mediaInput.enabled) return;
    uint16_t key = 0; NSUInteger mask = 0;
    switch (event.keyCode) {
        case kVK_Shift: key=160; mask=NX_DEVICELSHIFTKEYMASK; break;
        case kVK_RightShift: key=161; mask=NX_DEVICERSHIFTKEYMASK; break;
        case kVK_Control: key=162; mask=NX_DEVICELCTLKEYMASK; break;
        case kVK_RightControl: key=163; mask=NX_DEVICERCTLKEYMASK; break;
        case kVK_Option: key=164; mask=NX_DEVICELALTKEYMASK; break;
        case kVK_RightOption: key=165; mask=NX_DEVICERALTKEYMASK; break;
        case kVK_Command: key=91; mask=NX_DEVICELCMDKEYMASK; break;
        case kVK_RightCommand: key=92; mask=NX_DEVICERCMDKEYMASK; break;
        case kVK_CapsLock: mediaInput.emit(4,0,0,20,true); mediaInput.emit(4,0,0,20,false); return;
        default: return;
    }
    mediaInput.emit(4,0,0,key,(event.modifierFlags & mask)!=0);
}
- (void)mediaMouse:(NSEvent*)event kind:(uint8_t)kind button:(uint16_t)button down:(BOOL)down {
    if (!mediaInput.enabled) return;
    uint16_t x=0,y=0;
    if (!mediaInput.position(self,event,x,y)) return;
    if (kind == 2 && down) { [self.window makeFirstResponder:self]; mediaInput.buttons |= 1U<<button; }
    mediaInput.emit(kind,x,y,button,down);
    if (kind == 2 && !down) mediaInput.buttons &= ~(1U<<button);
}
- (void)mouseMoved:(NSEvent*)e { [self mediaMouse:e kind:1 button:0 down:NO]; }
- (void)mouseDragged:(NSEvent*)e { [self mouseMoved:e]; }
- (void)rightMouseDragged:(NSEvent*)e { [self mouseMoved:e]; }
- (void)otherMouseDragged:(NSEvent*)e { [self mouseMoved:e]; }
- (void)mouseDown:(NSEvent*)e { [self mediaMouse:e kind:2 button:0 down:YES]; }
- (void)mouseUp:(NSEvent*)e { [self mediaMouse:e kind:2 button:0 down:NO]; }
- (void)rightMouseDown:(NSEvent*)e { [self mediaMouse:e kind:2 button:1 down:YES]; }
- (void)rightMouseUp:(NSEvent*)e { [self mediaMouse:e kind:2 button:1 down:NO]; }
- (void)otherMouseDown:(NSEvent*)e { if (e.buttonNumber==2) [self mediaMouse:e kind:2 button:2 down:YES]; }
- (void)otherMouseUp:(NSEvent*)e { if (e.buttonNumber==2) [self mediaMouse:e kind:2 button:2 down:NO]; }
- (void)scrollWheel:(NSEvent*)event {
    if (!mediaInput.enabled) return;
    uint16_t x=0,y=0; if (!mediaInput.position(self,event,x,y)) return;
    // Preserve the existing wheel-unit contract. Trackpad fractional movement
    // accumulates rather than being rounded to zero on each event.
    if (!std::isfinite(event.scrollingDeltaY)) return;
    mediaInput.wheel = std::clamp(mediaInput.wheel + event.scrollingDeltaY *
        (event.hasPreciseScrollingDeltas ? 3 : 120),-32767.0,32767.0);
    const auto delta = int16_t(std::clamp(std::trunc(mediaInput.wheel),-32767.0,32767.0));
    if (delta) { mediaInput.wheel -= delta; mediaInput.emit(3,x,y,uint16_t(delta)); }
}
@end
