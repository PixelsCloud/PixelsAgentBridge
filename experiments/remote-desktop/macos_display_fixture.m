// Bounded 1080p pattern for actual remote compositor measurements. No input.
// Uses the same frame-code cells as linux_display_fixture.py and the Windows
// presentation sampler. Does not change display modes or capture user content.
#import <AppKit/AppKit.h>
#include <math.h>
#include <unistd.h>

static unsigned frameNumber;
@interface PatternView : NSView
@end
@implementation PatternView
- (BOOL)isFlipped { return YES; }
- (void)drawRect:(NSRect)dirty {
    for (int y=0;y<1080;y+=90) for (int x=0;x<1920;x+=120) {
        unsigned phase=(frameNumber*3+x/12+y/9)%128;
        [[NSColor colorWithCalibratedRed:phase/128.0 green:(128-phase)/128.0 blue:.35 alpha:1] setFill];
        NSRectFill(NSMakeRect(x,y,120,90));
    }
    for (int row=0;row<2;row++) for(int i=0;i<16;i++) {
        unsigned bit=i==0 || i==15 ? 1 : i==1 || i==14 ? 0 : (frameNumber>>(i-2))&1;
        [((bit^row) ? NSColor.whiteColor : NSColor.blackColor) setFill];
        NSRectFill(NSMakeRect(192+i*96,160+row*96,96,80));
    }
}
@end

int main(int argc,const char **argv) {
    if(argc!=2) return 2;
    @autoreleasepool {
        NSString *report=[NSString stringWithUTF8String:argv[1]];
        NSApplication *app=NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];[app finishLaunching];
        NSScreen *screen=NSScreen.screens.firstObject;
        if(screen.frame.size.width!=1920 || screen.frame.size.height!=1080 || screen.backingScaleFactor!=1) return 3;
        NSWindow *window=[[NSWindow alloc] initWithContentRect:screen.frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        window.title=@"Pixels generated 1080p presentation fixture";
        window.level=NSStatusWindowLevel+1;window.ignoresMouseEvents=YES;
        PatternView *view=[[PatternView alloc] initWithFrame:NSMakeRect(0,0,1920,1080)];window.contentView=view;
        [window orderFrontRegardless];[app activateIgnoringOtherApps:YES];
        double start=NSProcessInfo.processInfo.systemUptime,lastReport=0;
        while(NSProcessInfo.processInfo.systemUptime-start<120) {
            NSEvent *event=[app nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate dateWithTimeIntervalSinceNow:.005] inMode:NSDefaultRunLoopMode dequeue:YES];
            if(event)[app sendEvent:event];
            unsigned current=(unsigned)floor((NSProcessInfo.processInfo.systemUptime-start)*30);
            if(current!=frameNumber){frameNumber=current;[view setNeedsDisplay:YES];}
            [app updateWindows];
            double now=NSProcessInfo.processInfo.systemUptime;
            if(now-lastReport>.25) {
                NSDictionary *state=@{@"pid":@(getpid()),@"frame":@(frameNumber),@"width":@1920,@"height":@1080,@"updated":@(NSDate.date.timeIntervalSince1970)};
                NSData *data=[NSJSONSerialization dataWithJSONObject:state options:0 error:nil];
                if(![data writeToFile:report options:NSDataWritingAtomic error:nil])return 4;
                lastReport=now;
            }
        }
        [window orderOut:nil];
    }
    return 0;
}
