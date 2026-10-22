// The daemon's menu bar item (NSStatusItem) and the AppKit event pump that
// lets it work from the daemon's own main-thread loop. Compiled without ARC.
//
// Everything here runs on the main thread. Menu choices are reported through
// a C callback as (action, argument); the Rust side decides what they do.

#import <AppKit/AppKit.h>
#include <stdint.h>

typedef void (*TyloraStatusCallback)(void *context, int32_t action, int32_t argument);

enum { kTagStride = 1000 };

@interface TyloraStatusTarget : NSObject {
@public
    TyloraStatusCallback callback;
    void *context;
}
- (void)fire:(NSMenuItem *)item;
@end

@implementation TyloraStatusTarget
- (void)fire:(NSMenuItem *)item {
    NSInteger tag = item.tag;
    if (callback) callback(context, (int32_t)(tag / kTagStride), (int32_t)(tag % kTagStride));
}
@end

typedef struct {
    NSStatusItem *item;
    TyloraStatusTarget *target;
} TyloraStatus;

static void quad(NSBezierPath *path, CGFloat cx, CGFloat cy, CGFloat x, CGFloat y) {
    NSPoint p = path.currentPoint;
    [path curveToPoint:NSMakePoint(x, y)
         controlPoint1:NSMakePoint(p.x + 2.0 / 3.0 * (cx - p.x), p.y + 2.0 / 3.0 * (cy - p.y))
         controlPoint2:NSMakePoint(x + 2.0 / 3.0 * (cx - x), y + 2.0 / 3.0 * (cy - y))];
}

// The three parts of the Tylora "T" from icons/icon.svg, as a template image.
static NSImage *TyloraTemplateIcon(void) {
    NSImage *image = [NSImage imageWithSize:NSMakeSize(18, 18)
                                    flipped:YES
                             drawingHandler:^BOOL(NSRect rect) {
        (void)rect;
        // Glyph box in SVG units: x 92…412, y 115…462.
        CGFloat scale = 16.0 / 347.0;
        NSAffineTransform *transform = [NSAffineTransform transform];
        [transform translateXBy:(18.0 - 320.0 * scale) / 2.0 yBy:1.0];
        [transform scaleBy:scale];
        [transform translateXBy:-92.0 yBy:-115.0];

        NSBezierPath *left = [NSBezierPath bezierPath];
        [left moveToPoint:NSMakePoint(110, 115)];
        [left lineToPoint:NSMakePoint(248, 115)];
        quad(left, 256, 115, 256, 123);
        [left lineToPoint:NSMakePoint(256, 261)];
        [left lineToPoint:NSMakePoint(105.2, 206.7)];
        quad(left, 92, 202, 92, 188);
        [left lineToPoint:NSMakePoint(92, 133)];
        quad(left, 92, 115, 110, 115);
        [left closePath];

        NSBezierPath *right = [NSBezierPath bezierPath];
        [right moveToPoint:NSMakePoint(274, 115)];
        [right lineToPoint:NSMakePoint(394, 115)];
        quad(right, 412, 115, 412, 133);
        [right lineToPoint:NSMakePoint(412, 183)];
        quad(right, 412, 197, 399.5, 203.3);
        [right lineToPoint:NSMakePoint(266, 270)];
        [right lineToPoint:NSMakePoint(266, 123)];
        quad(right, 266, 115, 274, 115);
        [right closePath];

        NSBezierPath *stem = [NSBezierPath bezierPath];
        [stem moveToPoint:NSMakePoint(173.5, 243.7)];
        [stem lineToPoint:NSMakePoint(248.5, 270.3)];
        quad(stem, 256, 273, 256, 281);
        [stem lineToPoint:NSMakePoint(256, 454)];
        quad(stem, 256, 462, 248.5, 459.3);
        [stem lineToPoint:NSMakePoint(203.6, 443.5)];
        quad(stem, 166, 430, 166, 390);
        [stem lineToPoint:NSMakePoint(166, 249)];
        quad(stem, 166, 241, 173.5, 243.7);
        [stem closePath];

        [[NSColor blackColor] setFill];
        for (NSBezierPath *part in @[ left, right, stem ]) {
            [[transform transformBezierPath:part] fill];
        }
        return YES;
    }];
    image.template = YES;
    return image;
}

static void TyloraEnsureApplication(void) {
    static BOOL launched = NO;
    NSApplication *app = [NSApplication sharedApplication];
    if (!launched) {
        [app finishLaunching];
        launched = YES;
    }
}

void *tylora_status_create(TyloraStatusCallback callback, void *context) {
    @autoreleasepool {
        if (![NSThread isMainThread]) return NULL;
        TyloraEnsureApplication();
        TyloraStatus *status = calloc(1, sizeof(TyloraStatus));
        if (!status) return NULL;
        status->target = [[TyloraStatusTarget alloc] init];
        status->target->callback = callback;
        status->target->context = context;
        status->item = [[[NSStatusBar systemStatusBar] statusItemWithLength:NSVariableStatusItemLength] retain];
        status->item.button.image = TyloraTemplateIcon();
        status->item.button.imagePosition = NSImageLeft;
        status->item.button.toolTip = @"Tylora";
        status->item.menu = [[[NSMenu alloc] initWithTitle:@"Tylora"] autorelease];
        return status;
    }
}

static NSMenuItem *TyloraItem(TyloraStatus *status, NSString *title, int32_t action, int32_t argument) {
    NSMenuItem *item = [[[NSMenuItem alloc] initWithTitle:title action:@selector(fire:) keyEquivalent:@""] autorelease];
    item.target = status->target;
    item.tag = action * kTagStride + argument;
    return item;
}

// flags: bit 0 active, bit 1 occupied. Actions: 1 open app, 2 reconcile,
// 3 stop, 4 focus workspace `argument`, 5 toggle the workspace numbers.
void tylora_status_update(void *handle, const char *headline, int32_t show_digits,
                          const int32_t *ids, const char *const *labels, const uint8_t *flags,
                          int32_t count) {
    @autoreleasepool {
        TyloraStatus *status = handle;
        if (!status) return;

        NSMutableAttributedString *title = [[[NSMutableAttributedString alloc] init] autorelease];
        if (show_digits && count > 0) {
            NSFont *font = [NSFont monospacedDigitSystemFontOfSize:13 weight:NSFontWeightMedium];
            [title appendAttributedString:[[[NSAttributedString alloc] initWithString:@" "] autorelease]];
            for (int32_t index = 0; index < count; index++) {
                BOOL active = (flags[index] & 1) != 0;
                BOOL occupied = (flags[index] & 2) != 0;
                int32_t id = ids[index];
                // ❶…❾ (U+2776…) marks the active workspace.
                NSString *glyph = active && id >= 1 && id <= 9
                    ? [NSString stringWithFormat:@"%C", (unichar)(0x2775 + id)]
                    : [NSString stringWithFormat:@"%d", id];
                NSColor *color = active || occupied ? [NSColor labelColor] : [NSColor tertiaryLabelColor];
                NSString *text = index + 1 < count ? [glyph stringByAppendingString:@" "] : glyph;
                [title appendAttributedString:[[[NSAttributedString alloc]
                    initWithString:text
                        attributes:@{NSFontAttributeName : font, NSForegroundColorAttributeName : color}] autorelease]];
            }
        }
        status->item.button.attributedTitle = title;

        NSMenu *menu = status->item.menu;
        [menu removeAllItems];
        NSMenuItem *head = [[[NSMenuItem alloc] initWithTitle:[NSString stringWithUTF8String:headline ?: ""]
                                                       action:nil
                                                keyEquivalent:@""] autorelease];
        head.enabled = NO;
        [menu addItem:head];
        if (count > 0) {
            [menu addItem:[NSMenuItem separatorItem]];
            for (int32_t index = 0; index < count; index++) {
                NSString *label = [NSString stringWithUTF8String:labels[index] ?: ""];
                NSMenuItem *item = TyloraItem(status, label, 4, ids[index]);
                item.state = (flags[index] & 1) ? NSControlStateValueOn : NSControlStateValueOff;
                [menu addItem:item];
            }
        }
        [menu addItem:[NSMenuItem separatorItem]];
        NSMenuItem *digits = TyloraItem(status, @"Show Workspace Numbers", 5, 0);
        digits.state = show_digits ? NSControlStateValueOn : NSControlStateValueOff;
        [menu addItem:digits];
        [menu addItem:TyloraItem(status, @"Open Tylora…", 1, 0)];
        [menu addItem:TyloraItem(status, @"Reconcile Windows", 2, 0)];
        [menu addItem:[NSMenuItem separatorItem]];
        [menu addItem:TyloraItem(status, @"Stop Tylora", 3, 0)];
    }
}

void tylora_status_destroy(void *handle) {
    @autoreleasepool {
        TyloraStatus *status = handle;
        if (!status) return;
        [[NSStatusBar systemStatusBar] removeStatusItem:status->item];
        [status->item release];
        status->target->callback = NULL;
        [status->target release];
        free(status);
    }
}

// Dispatches AppKit events for up to `seconds` (returns early once events
// were handled). Menus track inside `sendEvent:` until they close.
void tylora_pump_events(double seconds) {
    @autoreleasepool {
        if (![NSThread isMainThread]) return;
        TyloraEnsureApplication();
        NSApplication *app = NSApp;
        NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
        for (;;) {
            NSEvent *event = [app nextEventMatchingMask:NSEventMaskAny
                                              untilDate:until
                                                 inMode:NSDefaultRunLoopMode
                                                dequeue:YES];
            if (!event) break;
            [app sendEvent:event];
            until = [NSDate distantPast];
        }
    }
}
