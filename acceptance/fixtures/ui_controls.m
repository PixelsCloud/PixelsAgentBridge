#import <Cocoa/Cocoa.h>
#include <unistd.h>

@interface PabControls : NSObject <NSTableViewDataSource,NSTableViewDelegate>
@property(copy) NSString *directory;
@property(strong) NSTextField *input;
@property(strong) NSButton *check;
@property(strong) NSButton *radio;
@property(strong) NSTableView *table;
@property NSInteger clicks;
-(void)apply:(id)sender;
@end
@implementation PabControls
-(NSInteger)numberOfRowsInTableView:(NSTableView*)table {return 2;}
-(id)tableView:(NSTableView*)table objectValueForTableColumn:(NSTableColumn*)column row:(NSInteger)row {
    return row==0 ? @"Fixture first" : @"Fixture second";
}
-(NSTableRowView*)tableView:(NSTableView*)table rowViewForRow:(NSInteger)row {
    NSTableRowView *view=[NSTableRowView new];
    view.accessibilityLabel=row==0 ? @"Fixture first row" : @"Fixture second row";
    return view;
}
-(void)apply:(id)sender {
    self.clicks++;
    NSDictionary *result=@{@"clicks":@(self.clicks),@"value":self.input.stringValue,
                           @"checked":@(self.check.state==NSControlStateValueOn),
                           @"radio":@(self.radio.state==NSControlStateValueOn),
                           @"selected":@(self.table.selectedRow)};
    NSData *json=[NSJSONSerialization dataWithJSONObject:result options:0 error:nil];
    [json writeToFile:[self.directory stringByAppendingPathComponent:@"result.json"] atomically:YES];
}
@end

// Keep the window/controller alive throughout NSApplication.run under ARC.
static NSWindow *fixtureWindow;
static PabControls *fixtureTarget;

int main(int argc,char **argv) {
    if(argc<2 || argc>3)return 2;
    @autoreleasepool {
        NSString *directory=[NSString stringWithUTF8String:argv[1]];
        BOOL isDirectory=NO;
        NSFileManager *files=[NSFileManager defaultManager];
        if(![files fileExistsAtPath:directory isDirectory:&isDirectory] || !isDirectory ||
           ![files isWritableFileAtPath:directory]) {
            fprintf(stderr,"Create a fresh writable fixture directory first\n");
            return 5;
        }
        NSString *ready=[directory stringByAppendingPathComponent:@"ready.json"];
        if([[NSFileManager defaultManager] fileExistsAtPath:ready])return 3;
        NSApplication *app=[NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        dispatch_async(dispatch_get_main_queue(), ^{
        NSWindow *win=[[NSWindow alloc] initWithContentRect:NSMakeRect(400,100,600,540)
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
        NSTextField *readonly=[[NSTextField alloc] initWithFrame:NSMakeRect(20,460,220,30)];
        readonly.accessibilityLabel=@"Fixture readonly";readonly.stringValue=@"unchanged";readonly.editable=NO;
        [win.contentView addSubview:readonly];
        NSSecureTextField *secure=[[NSSecureTextField alloc] initWithFrame:NSMakeRect(20,420,220,30)];
        secure.accessibilityLabel=@"Fixture secure";secure.stringValue=@"fixture-only-secret";
        [win.contentView addSubview:secure];
        NSButton *disabled=[[NSButton alloc] initWithFrame:NSMakeRect(20,380,220,30)];
        disabled.title=@"Fixture disabled";disabled.enabled=NO;
        [win.contentView addSubview:disabled];
        target.radio=[[NSButton alloc] initWithFrame:NSMakeRect(20,340,220,30)];
        [target.radio setButtonType:NSButtonTypeRadio];target.radio.title=@"Fixture radio";
        [win.contentView addSubview:target.radio];
        target.table=[[NSTableView alloc] initWithFrame:NSMakeRect(300,200,220,100)];
        NSTableColumn *column=[[NSTableColumn alloc] initWithIdentifier:@"fixture_column"];
        column.width=200;[target.table addTableColumn:column];
        target.table.headerView=nil;target.table.dataSource=target;target.table.delegate=target;
        target.table.accessibilityLabel=@"Fixture list";
        NSScrollView *scroll=[[NSScrollView alloc] initWithFrame:target.table.frame];
        scroll.documentView=target.table;[win.contentView addSubview:scroll];
        [target.table reloadData];
        for(int i=0;i<2;i++){
            NSButton *duplicate=[[NSButton alloc] initWithFrame:NSMakeRect(300,340+i*40,220,30)];
            duplicate.title=@"Fixture duplicate";[win.contentView addSubview:duplicate];
        }
        int nodeCount=argc==3 ? atoi(argv[2]) : 0;
        if(nodeCount<0 || nodeCount>1000)exit(4);
        for(int i=0;i<nodeCount;i++){
            NSTextField *label=[NSTextField labelWithString:[NSString stringWithFormat:@"Budget node %d",i]];
            label.frame=NSMakeRect(300,450+i*22,200,20);[win.contentView addSubview:label];
        }
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
