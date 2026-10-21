#pragma once
// AppKit composites this local control above the Metal surface; no pixel copy.
@interface PABViewerStats : NSButton
@property(nonatomic, copy) NSString* statisticsText;
- (void)placeInViewer;
@end
@implementation PABViewerStats
- (instancetype)initWithFrame:(NSRect)frame {
    if ((self = [super initWithFrame:frame])) {
        self.bordered = NO; self.wantsLayer = YES;
        self.target = self; self.action = @selector(releaseViewerInput:);
        self.layer.cornerRadius = 9;
        self.layer.backgroundColor = [NSColor colorWithCalibratedRed:.08 green:.12 blue:.16 alpha:.84].CGColor;
        self.toolTip = nil;
        [self setAccessibilityRole:NSAccessibilityStaticTextRole];
    }
    return self;
}
- (BOOL)isOpaque { return NO; }
- (BOOL)acceptsFirstResponder { return NO; }
- (void)placeInViewer {
    NSRect bounds = self.superview.bounds;
    const auto lines = [self.statisticsText componentsSeparatedByString:@"\n"].count;
    const CGFloat width = std::min(360.0,std::max(1.0,bounds.size.width-24));
    const CGFloat height = std::min(24.0+lines*19,std::max(1.0,bounds.size.height-24));
    self.frame = NSMakeRect(std::max(0.0,bounds.size.width-width-12),
        self.superview.flipped ? std::max(0.0,bounds.size.height-height-12) : 12,width,height);
}
- (void)drawRect:(NSRect)dirty {
    NSDictionary* attributes = @{NSFontAttributeName:[NSFont systemFontOfSize:12],NSForegroundColorAttributeName:NSColor.whiteColor};
    [self.statisticsText drawInRect:NSInsetRect(self.bounds,12,10) withAttributes:attributes];
}
- (void)releaseViewerInput:(id)sender {
    if ([self.superview isKindOfClass:PABInputView.class]) ((PABInputView*)self.superview)->mediaInput.release();
    // Only the titlebar Debug button changes visibility.
}
@end
