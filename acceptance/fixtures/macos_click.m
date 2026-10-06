#import <Cocoa/Cocoa.h>

// Compile with clang -framework Cocoa. Run in the signed-in user's GUI session.
// argv[1] is a fresh result file inside an owned acceptance directory.
@interface ClickTarget : NSObject
@property(copy) NSString *resultPath;
-(void)clicked:(id)sender;
@end
@implementation ClickTarget
-(void)clicked:(id)sender {
    NSError *error = nil;
    BOOL saved = [@"clicked" writeToFile:self.resultPath atomically:YES
                              encoding:NSUTF8StringEncoding error:&error];
    [(NSButton *)sender setTitle:saved ? @"PASS: click received" : @"FAIL: marker write"];
}
@end

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *win = [[NSWindow alloc] initWithContentRect:NSMakeRect(800,300,400,200)
            styleMask:(NSWindowStyleMaskTitled|NSWindowStyleMaskClosable)
            backing:NSBackingStoreBuffered defer:NO];
        [win setTitle:@"PAB monitor acceptance fixture"];
        ClickTarget *target = [ClickTarget new];
        target.resultPath = [NSString stringWithUTF8String:argv[1]];
        NSButton *button = [[NSButton alloc] initWithFrame:NSMakeRect(100,50,200,60)];
        [button setTitle:@"Click acceptance target"];
        [button setBezelStyle:NSBezelStyleRounded];
        [button setTarget:target];
        [button setAction:@selector(clicked:)];
        [[win contentView] addSubview:button];
        [win makeKeyAndOrderFront:nil];
        [app activateIgnoringOtherApps:YES];
        [NSTimer scheduledTimerWithTimeInterval:240 repeats:NO
            block:^(NSTimer *timer){ [app terminate:nil]; }];
        [app run];
    }
    return 0;
}
