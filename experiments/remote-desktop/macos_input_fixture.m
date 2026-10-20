// An owned AppKit target for the opt-in authenticated media-helper test.
// Observes only events delivered to its own view; never installs an event tap.
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include <math.h>
#include <unistd.h>

static NSString *reportPath;
static NSUInteger downs, ups, keysDown, keysUp, wheels, shiftDown, shiftUp;
static BOOL shifted;
static NSPoint lastClick;
static unsigned short lastKey;

@interface FixtureView : NSView
@end
@implementation FixtureView
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)acceptsFirstMouse:(NSEvent *)event { return YES; }
- (void)drawRect:(NSRect)rect {
    [[NSColor colorWithCalibratedRed:0.12 green:0.45 blue:0.4 alpha:1] setFill];
    NSRectFill(self.bounds);
    [@"Pixels input test — owned window" drawAtPoint:NSMakePoint(20, 200)
        withAttributes:@{NSForegroundColorAttributeName:NSColor.whiteColor,
                         NSFontAttributeName:[NSFont systemFontOfSize:20]}];
}
- (void)mouseDown:(NSEvent *)event { downs++; lastClick=[self convertPoint:event.locationInWindow fromView:nil]; }
- (void)mouseUp:(NSEvent *)event { ups++; }
- (void)keyDown:(NSEvent *)event { keysDown++; lastKey=event.keyCode; }
- (void)keyUp:(NSEvent *)event { keysUp++; }
- (void)scrollWheel:(NSEvent *)event { wheels++; }
- (void)flagsChanged:(NSEvent *)event {
    BOOL next=(event.modifierFlags & NSEventModifierFlagShift)!=0;
    if (next!=shifted) { if (next) shiftDown++; else shiftUp++; shifted=next; }
}
@end

int main(int argc, const char **argv) {
    if (argc!=2) return 2;
    @autoreleasepool {
        reportPath=[NSString stringWithUTF8String:argv[1]];
        NSApplication *app=NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        [app finishLaunching];
        NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(160,300,500,260)
            styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        window.title=@"Pixels native input acceptance";
        FixtureView *view=[[FixtureView alloc] initWithFrame:NSMakeRect(0,0,500,260)];
        window.contentView=view;
        [window makeFirstResponder:view];
        [window makeKeyAndOrderFront:nil];
        [app activateIgnoringOtherApps:YES];
        NSDate *deadline=[NSDate dateWithTimeIntervalSinceNow:120];
        pid_t lastFront=-1;
        while (deadline.timeIntervalSinceNow>0) {
            NSEvent *event=[app nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]
                inMode:NSDefaultRunLoopMode dequeue:YES];
            if(event) [app sendEvent:event];
            [app updateWindows];
            NSScreen *screen=window.screen;
            CGDirectDisplayID display=[screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
            CGRect bounds=CGDisplayBounds(display);
            NSPoint target=[window convertPointToScreen:[view convertPoint:NSMakePoint(100,100) toView:nil]];
            double globalY=NSScreen.screens.firstObject.frame.size.height-target.y;
            double nx=(target.x-bounds.origin.x)*65535.0/(bounds.size.width-1);
            double ny=(globalY-bounds.origin.y)*65535.0/(bounds.size.height-1);
            NSRunningApplication *front=NSWorkspace.sharedWorkspace.frontmostApplication;
            if (front.processIdentifier!=lastFront) {
                fprintf(stderr,"front_pid=%d bundle=%s policy=%ld active=%d key=%d\n",front.processIdentifier,
                    (front.bundleIdentifier ?: @"unbundled").UTF8String,(long)front.activationPolicy,app.active,window.keyWindow);
                fflush(stderr);lastFront=front.processIdentifier;
            }
            NSDictionary *record=@{@"pid":@(getpid()), @"focused":@((BOOL)(window.keyWindow && app.active)),
                @"timestamp":@(NSDate.date.timeIntervalSince1970), @"screen_id":[NSString stringWithFormat:@"%u",display],
                @"x":@(lround(nx)), @"y":@(lround(ny)), @"downs":@(downs), @"ups":@(ups),
                @"keys_down":@(keysDown), @"keys_up":@(keysUp), @"last_key":@(lastKey), @"wheels":@(wheels),
                @"shift_down":@(shiftDown), @"shift_up":@(shiftUp), @"shifted":@(shifted),
                @"click_x":@(lastClick.x), @"click_y":@(lastClick.y)};
            NSData *json=[NSJSONSerialization dataWithJSONObject:record options:0 error:nil];
            if(![json writeToFile:reportPath options:NSDataWritingAtomic error:nil]) return 3;
        }
    }
    return 0;
}
