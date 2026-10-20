#import <AppKit/AppKit.h>
int main(void) { @autoreleasepool {
 NSApplication *app=[NSApplication sharedApplication];
 NSInteger before=app.activationPolicy;
 BOOL changed=[app setActivationPolicy:NSApplicationActivationPolicyProhibited];
 NSInteger after=app.activationPolicy;
 printf("{\"before\":%ld,\"setter_returned\":%s,\"after\":%ld,\"prohibited\":%s}\n",(long)before,changed?"true":"false",(long)after,after==NSApplicationActivationPolicyProhibited?"true":"false");
 return after==NSApplicationActivationPolicyProhibited?0:1;
}}
