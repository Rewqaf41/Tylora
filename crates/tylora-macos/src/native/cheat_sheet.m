// The hotkey cheat sheet: a translucent panel, centred on the display under
// the pointer, that never takes focus or clicks. Main thread only; compiled
// without ARC.

#import <AppKit/AppKit.h>
#include <stdint.h>

static NSPanel *sheet;

static NSTextField *TyloraLabel(NSString *text, NSFont *font, NSColor *color) {
    NSTextField *field = [NSTextField labelWithString:text];
    field.font = font;
    field.textColor = color;
    // Labels keep their full width; the panel grows instead of clipping.
    [field setContentCompressionResistancePriority:NSLayoutPriorityRequired
                                    forOrientation:NSLayoutConstraintOrientationHorizontal];
    return field;
}

// One group: a heading over a two-column grid of chords and actions.
static NSView *TyloraColumn(NSString *title, const char *const *chords,
                            const char *const *labels, int32_t count) {
    NSMutableArray<NSArray<NSView *> *> *rows = [NSMutableArray array];
    NSFont *chordFont = [NSFont monospacedSystemFontOfSize:13 weight:NSFontWeightMedium];
    NSFont *labelFont = [NSFont systemFontOfSize:13];
    for (int32_t index = 0; index < count; index++) {
        [rows addObject:@[
            TyloraLabel([NSString stringWithUTF8String:chords[index] ?: ""], chordFont, [NSColor labelColor]),
            TyloraLabel([NSString stringWithUTF8String:labels[index] ?: ""], labelFont, [NSColor labelColor]),
        ]];
    }
    NSGridView *grid = [NSGridView gridViewWithViews:rows];
    grid.rowSpacing = 6;
    grid.columnSpacing = 14;
    NSTextField *heading = TyloraLabel(title.uppercaseString,
                                       [NSFont systemFontOfSize:11 weight:NSFontWeightSemibold],
                                       [NSColor secondaryLabelColor]);
    NSStackView *column = [NSStackView stackViewWithViews:@[ heading, grid ]];
    column.orientation = NSUserInterfaceLayoutOrientationVertical;
    column.alignment = NSLayoutAttributeLeading;
    column.spacing = 8;
    // Rows stay packed at the top instead of spreading over the tallest column.
    for (NSView *view in @[ grid, column ]) {
        [view setContentHuggingPriority:NSLayoutPriorityRequired
                         forOrientation:NSLayoutConstraintOrientationVertical];
    }
    return column;
}

void tylora_cheat_sheet_show(int32_t group_count, const char *const *titles, const int32_t *row_counts,
                             const char *const *chords, const char *const *labels) {
    @autoreleasepool {
        if (![NSThread isMainThread] || group_count <= 0) return;
        if (!sheet) {
            sheet = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 100, 100)
                                               styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                                                 backing:NSBackingStoreBuffered
                                                   defer:YES];
            sheet.level = NSPopUpMenuWindowLevel;
            sheet.opaque = NO;
            sheet.backgroundColor = [NSColor clearColor];
            sheet.hasShadow = YES;
            sheet.ignoresMouseEvents = YES;
            sheet.hidesOnDeactivate = NO;
            sheet.releasedWhenClosed = NO;
            sheet.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                       NSWindowCollectionBehaviorFullScreenAuxiliary |
                                       NSWindowCollectionBehaviorTransient |
                                       NSWindowCollectionBehaviorIgnoresCycle;
        }
        NSVisualEffectView *background = [[[NSVisualEffectView alloc] init] autorelease];
        background.material = NSVisualEffectMaterialHUDWindow;
        background.blendingMode = NSVisualEffectBlendingModeBehindWindow;
        background.state = NSVisualEffectStateActive;
        background.wantsLayer = YES;
        background.layer.cornerRadius = 16;
        background.layer.masksToBounds = YES;

        NSTextField *title = [NSTextField labelWithString:@"Tylora shortcuts"];
        title.font = [NSFont systemFontOfSize:15 weight:NSFontWeightSemibold];
        NSStackView *columns = [[[NSStackView alloc] init] autorelease];
        columns.orientation = NSUserInterfaceLayoutOrientationHorizontal;
        columns.alignment = NSLayoutAttributeTop;
        columns.spacing = 36;
        int32_t offset = 0;
        for (int32_t group = 0; group < group_count; group++) {
            NSString *name = [NSString stringWithUTF8String:titles[group] ?: ""];
            [columns addArrangedSubview:TyloraColumn(name, chords + offset, labels + offset, row_counts[group])];
            offset += row_counts[group];
        }
        NSTextField *hint = [NSTextField labelWithString:@"Release ⌥ to close"];
        hint.font = [NSFont systemFontOfSize:11];
        hint.textColor = [NSColor tertiaryLabelColor];
        NSStackView *content = [NSStackView stackViewWithViews:@[ title, columns, hint ]];
        content.orientation = NSUserInterfaceLayoutOrientationVertical;
        content.alignment = NSLayoutAttributeLeading;
        content.spacing = 16;
        content.edgeInsets = NSEdgeInsetsMake(22, 26, 18, 26);
        content.translatesAutoresizingMaskIntoConstraints = NO;
        [background addSubview:content];
        [NSLayoutConstraint activateConstraints:@[
            [content.leadingAnchor constraintEqualToAnchor:background.leadingAnchor],
            [content.trailingAnchor constraintEqualToAnchor:background.trailingAnchor],
            [content.topAnchor constraintEqualToAnchor:background.topAnchor],
            [content.bottomAnchor constraintEqualToAnchor:background.bottomAnchor],
        ]];
        sheet.contentView = background;
        [background layoutSubtreeIfNeeded];
        NSSize size = content.fittingSize;
        // NSStackView may report less than its columns need; sum them.
        CGFloat width = content.edgeInsets.left + content.edgeInsets.right;
        for (NSView *column in columns.arrangedSubviews) width += column.fittingSize.width;
        width += columns.spacing * MAX(0, (CGFloat)columns.arrangedSubviews.count - 1);
        size.width = MAX(size.width, width);

        NSScreen *screen = NSScreen.mainScreen;
        NSPoint pointer = NSEvent.mouseLocation;
        for (NSScreen *candidate in NSScreen.screens) {
            if (NSPointInRect(pointer, candidate.frame)) screen = candidate;
        }
        NSRect area = screen.visibleFrame;
        size.width = MIN(size.width, area.size.width - 40);
        size.height = MIN(size.height, area.size.height - 40);
        NSRect frame = NSMakeRect(NSMidX(area) - size.width / 2, NSMidY(area) - size.height / 2,
                                  size.width, size.height);
        [sheet setFrame:frame display:YES];
        sheet.alphaValue = 0;
        [sheet orderFrontRegardless];
        [NSAnimationContext runAnimationGroup:^(NSAnimationContext *context) {
            context.duration = 0.15;
            sheet.animator.alphaValue = 1;
        }];
    }
}

void tylora_cheat_sheet_hide(void) {
    @autoreleasepool {
        if (![NSThread isMainThread] || !sheet) return;
        [sheet orderOut:nil];
        sheet.contentView = nil;
    }
}
