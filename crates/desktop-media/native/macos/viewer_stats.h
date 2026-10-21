#pragma once
// AppKit composites this local control above the Metal surface; no pixel copy.
@interface PABViewerStats : NSButton
@property(nonatomic, copy) NSString* statisticsText;
@property(nonatomic) BOOL expanded;
- (void)placeInViewer;
@end
@implementation PABViewerStats
- (instancetype)initWithFrame:(NSRect)frame {
    if ((self = [super initWithFrame:frame])) {
        self.expanded = YES; self.bordered = NO; self.wantsLayer = YES;
        self.target = self; self.action = @selector(toggleStatistics:);
        self.layer.cornerRadius = 9;
        self.layer.backgroundColor = [NSColor colorWithCalibratedRed:.08 green:.12 blue:.16 alpha:.84].CGColor;
        self.toolTip = @"Show / hide statistics";
        [self setAccessibilityRole:NSAccessibilityButtonRole];
    }
    return self;
}
- (BOOL)isOpaque { return NO; }
- (BOOL)acceptsFirstResponder { return NO; }
- (void)placeInViewer {
    NSRect bounds = self.superview.bounds;
    const auto lines = [self.statisticsText componentsSeparatedByString:@"\n"].count;
    const CGFloat width = std::min(self.expanded ? 244.0 : 30.0,std::max(1.0,bounds.size.width-24));
    const CGFloat height = std::min(self.expanded ? 24.0+lines*19 : 30.0,std::max(1.0,bounds.size.height-24));
    self.frame = NSMakeRect(std::max(0.0,bounds.size.width-width-12),
        self.superview.flipped ? std::max(0.0,bounds.size.height-height-12) : 12,width,height);
}
- (void)drawRect:(NSRect)dirty {
    NSString* text = self.expanded ? self.statisticsText : @"i";
    NSDictionary* attributes = @{NSFontAttributeName:[NSFont systemFontOfSize:12],NSForegroundColorAttributeName:NSColor.whiteColor};
    if (self.expanded) [text drawInRect:NSInsetRect(self.bounds,12,10) withAttributes:attributes];
    else {
        NSSize size = [text sizeWithAttributes:attributes];
        [text drawAtPoint:NSMakePoint((self.bounds.size.width-size.width)/2,(self.bounds.size.height-size.height)/2) withAttributes:attributes];
    }
}
- (void)toggleStatistics:(id)sender {
    if ([self.superview isKindOfClass:PABInputView.class]) ((PABInputView*)self.superview)->mediaInput.release();
    self.expanded = !self.expanded; [self placeInViewer]; self.needsDisplay = YES;
}
@end
