#import <Cocoa/Cocoa.h>
#include <unistd.h>

@interface PabControls : NSObject
@property(copy) NSString *directory;
@property(strong) NSTextField *input;
@property(strong) NSButton *check;
@property NSInteger clicks;
-(void)apply:(id)sender;
@end
@implementation PabControls
-(void)apply:(id)sender {
    self.clicks++;
    NSDictionary *result=@{@"clicks":@(self.clicks),@"value":self.input.stringValue,
                           @"checked":@(self.check.state==NSControlStateValueOn)};
    NSData *json=[NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    [json writeToFile:[self.directory stringByAppendingPathComponent:@"result.json"] atomically:YES];
}
@end

// Keep the window/controller alive throughout NSApplication.run under ARC.
static NSWindow *fixtureWindow;
static PabControls *fixtureTarget;

int main(int argc,char **argv) {
    if(argc!=2)return 2;
    @autoreleasepool {
        NSString *directory=[NSString stringWithUTF8String:argv[1]];
        NSString *ready=[directory stringByAppendingPathComponent:@"ready.json"];
        if([[NSFileManager defaultManager] fileExistsAtPath:ready])return 3;
        NSApplication *app=[NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        dispatch_async(dispatch_get_main_queue(), ^{
        NSWindow *win=[[NSWindow alloc] initWithContentRect:NSMakeRect(800,300,440,260)
            styleMask:(NSWindowStyleMaskTitled|NSWindowStyleMaskClosable) backing:NSBackingStoreBuffered defer:NO];
        win.title=@"PAB UI acceptance fixture";
        PabControls *target=[PabControls new];target.directory=directory;
        fixtureWindow=win;fixtureTarget=target;
        target.input=[[NSTextField alloc] initWithFrame:NSMakeRect(20,200,360,30)];
        target.input.accessibilityLabel=@"Fixture input";
        target.input.accessibilityIdentifier=@"fixture_input";
        [win.contentView addSubview:target.input];
        target.check=[[NSButton alloc] initWithFrame:NSMakeRect(20,150,200,30)];
        [target.check setButtonType:NSButtonTypeSwitch];target.check.title=@"Fixture option";
        [win.contentView addSubview:target.check];
        NSButton *button=[[NSButton alloc] initWithFrame:NSMakeRect(20,90,200,40)];
        button.title=@"Apply fixture";button.target=target;button.action=@selector(apply:);
        [win.contentView addSubview:button];
        [win makeKeyAndOrderFront:nil];[app activateIgnoringOtherApps:YES];
        NSAccessibilityPostNotification(win,NSAccessibilityWindowCreatedNotification);
        NSData *json=[NSJSONSerialization dataWithJSONObject:@{@"pid":@(getpid()),@"window_role":win.accessibilityRole ?: @"",@"input_role":target.input.accessibilityRole ?: @"",@"window_number":@(win.windowNumber)} options:0 error:nil];
        [json writeToFile:ready atomically:YES];
        [NSTimer scheduledTimerWithTimeInterval:0.1 repeats:YES block:^(NSTimer *t){
            NSFileManager *fm=[NSFileManager defaultManager];
            if([fm fileExistsAtPath:[directory stringByAppendingPathComponent:@"stop"]])[app terminate:nil];
            NSString *hang=[directory stringByAppendingPathComponent:@"hang"];
            if([fm fileExistsAtPath:hang]){[fm removeItemAtPath:hang error:nil];[NSThread sleepForTimeInterval:10];}
        }];
        [NSTimer scheduledTimerWithTimeInterval:300 repeats:NO block:^(NSTimer *t){[app terminate:nil];}];
        });
        [app run];
    }
    return 0;
}
