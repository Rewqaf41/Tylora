#import <ApplicationServices/ApplicationServices.h>
#import <CoreFoundation/CoreFoundation.h>
#import <Foundation/Foundation.h>
#import <dlfcn.h>
#import <mach-o/dyld.h>
#import <mach-o/loader.h>
#import <mach-o/nlist.h>
#import <objc/message.h>
#import <objc/runtime.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>

static const char *move_symbol = "__ZL54SLSPerformAsynchronousBridgedWindowManagementOperationP47SLSAsynchronousBridgedWindowManagementOperation";
static pthread_once_t sky_once = PTHREAD_ONCE_INIT;
static pthread_once_t his_once = PTHREAD_ONCE_INIT;
static void *sky_handle;
static void *his_handle;
static void open_sky(void) { sky_handle = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", RTLD_LAZY); }
static void open_his(void) { his_handle = dlopen("/System/Library/Frameworks/ApplicationServices.framework/Frameworks/HIServices.framework/HIServices", RTLD_LAZY); }

static void *local_skylight_symbol(void) {
    for (uint32_t i = 0; i < _dyld_image_count(); i++) {
        const char *path = _dyld_get_image_name(i);
        if (!path || !strstr(path, "/SkyLight.framework/")) continue;
        const struct mach_header_64 *header = (const void *)_dyld_get_image_header(i);
        if (!header || header->magic != MH_MAGIC_64) return NULL;
        const uint8_t *cursor = (const uint8_t *)(header + 1);
        const uint8_t *end = cursor + header->sizeofcmds;
        const struct segment_command_64 *linkedit = NULL;
        const struct symtab_command *symtab = NULL;
        for (uint32_t n = 0; n < header->ncmds; n++) {
            if (cursor + sizeof(struct load_command) > end) return NULL;
            const struct load_command *command = (const void *)cursor;
            if (command->cmdsize < sizeof(*command) || cursor + command->cmdsize > end) return NULL;
            if (command->cmd == LC_SEGMENT_64 && command->cmdsize >= sizeof(struct segment_command_64)) {
                const struct segment_command_64 *segment = (const void *)cursor;
                if (!strncmp(segment->segname, SEG_LINKEDIT, 16)) linkedit = segment;
            } else if (command->cmd == LC_SYMTAB && command->cmdsize >= sizeof(struct symtab_command)) {
                symtab = (const void *)cursor;
            }
            cursor += command->cmdsize;
        }
        if (!linkedit || !symtab) return NULL;
        uintptr_t slide = _dyld_get_image_vmaddr_slide(i);
        uintptr_t base = linkedit->vmaddr - linkedit->fileoff + slide;
        const struct nlist_64 *symbols = (const void *)(base + symtab->symoff);
        const char *strings = (const void *)(base + symtab->stroff);
        for (uint32_t n = 0; n < symtab->nsyms; n++) {
            uint32_t offset = symbols[n].n_un.n_strx;
            if (offset >= symtab->strsize) continue;
            size_t available = symtab->strsize - offset;
            if (!memchr(strings + offset, 0, available)) continue;
            if (!strcmp(strings + offset, move_symbol)) return (void *)(symbols[n].n_value + slide);
        }
    }
    return NULL;
}

// 0 = available; 1 = SkyLight absent; 2 = static symbol absent;
// 3 = ObjC class/selector absent; 4 = AX identity function absent.
int tylora_move_bridge_probe(void) {
    pthread_once(&sky_once, open_sky);
    if (!sky_handle) return 1;
    if (!local_skylight_symbol()) return 2;
    Class cls = objc_getClass("SLSBridgedMoveWindowsToManagedSpaceOperation");
    Method initializer = cls ? class_getInstanceMethod(cls, sel_registerName("initWithWindows:spaceID:")) : NULL;
    const char *encoding = initializer ? method_getTypeEncoding(initializer) : NULL;
    // macOS 26.3 arm64: @32@0:8@16Q24. Fail closed if the ABI changes.
    if (!encoding || strcmp(encoding, "@32@0:8@16Q24")) return 3;
    pthread_once(&his_once, open_his);
    if (!his_handle || !dlsym(his_handle, "_AXUIElementGetWindow")) return 4;
    return 0;
}

// Focused AX window -> verified CGWindowID. Never guesses by title or geometry.
// Returns AXError, or -1 for missing symbol/invalid identity.
int tylora_focused_cg_window(uint32_t *wid, int32_t *pid) {
    if (!wid || !pid) return -1;
    *wid = 0; *pid = 0;
    pthread_once(&his_once, open_his);
    AXError (*get_window)(AXUIElementRef, uint32_t *) = his_handle ? dlsym(his_handle, "_AXUIElementGetWindow") : NULL;
    if (!get_window) return -1;
    AXUIElementRef system = AXUIElementCreateSystemWide();
    if (!system) return -1;
    AXUIElementRef app = NULL;
    AXUIElementRef window = NULL;
    AXError error = AXUIElementCopyAttributeValue(system, kAXFocusedApplicationAttribute, (CFTypeRef *)&app);
    if (error == kAXErrorSuccess && app) error = AXUIElementGetPid(app, (pid_t *)pid);
    if (error == kAXErrorSuccess && app) error = AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute, (CFTypeRef *)&window);
    if (error == kAXErrorSuccess && window) error = get_window(window, wid);
    if (window) CFRelease(window);
    if (app) CFRelease(app);
    CFRelease(system);
    if (error == kAXErrorSuccess && (!*wid || *pid <= 0)) return -1;
    return error;
}

// Focused (else main) window of one application. Used when the caller has
// already resolved the frontmost pid without the system-wide element.
int tylora_app_focused_cg_window(int32_t pid, uint32_t *wid) {
    if (!wid || pid <= 0) return -1;
    *wid = 0;
    pthread_once(&his_once, open_his);
    AXError (*get_window)(AXUIElementRef, uint32_t *) = his_handle ? dlsym(his_handle, "_AXUIElementGetWindow") : NULL;
    if (!get_window) return -1;
    AXUIElementRef app = AXUIElementCreateApplication((pid_t)pid);
    if (!app) return -1;
    AXUIElementRef window = NULL;
    AXError error = AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute, (CFTypeRef *)&window);
    if (error != kAXErrorSuccess || !window) {
        if (window) { CFRelease(window); window = NULL; }
        error = AXUIElementCopyAttributeValue(app, kAXMainWindowAttribute, (CFTypeRef *)&window);
    }
    if (error == kAXErrorSuccess && window) error = get_window(window, wid);
    if (window) CFRelease(window);
    CFRelease(app);
    if (error == kAXErrorSuccess && !*wid) return -1;
    return error;
}

int tylora_ax_cg_window(const void *element, uint32_t *wid) {
    if (!element || !wid) return -1;
    *wid = 0;
    pthread_once(&his_once, open_his);
    AXError (*get_window)(AXUIElementRef, uint32_t *) = his_handle ? dlsym(his_handle, "_AXUIElementGetWindow") : NULL;
    if (!get_window) return -1;
    return get_window((AXUIElementRef)element, wid);
}

// Submission only. The Rust wrapper must verify SLSCopySpacesForWindows.
// 0 = submitted, other values are probe errors; 5 = allocation failed.
int tylora_submit_window_move(uint32_t wid, uint64_t target) {
    if (!wid || !target) return 5;
    int probe = tylora_move_bridge_probe();
    if (probe) return probe;
    @autoreleasepool {
        int64_t (*perform)(void *) = local_skylight_symbol();
        Class cls = objc_getClass("SLSBridgedMoveWindowsToManagedSpaceOperation");
        int32_t signed_id = (int32_t)wid;
        CFNumberRef number = CFNumberCreate(NULL, kCFNumberSInt32Type, &signed_id);
        if (!number) return 5;
        const void *items[] = { number };
        CFArrayRef windows = CFArrayCreate(NULL, items, 1, &kCFTypeArrayCallBacks);
        if (!windows) { CFRelease(number); return 5; }
        id operation = ((id (*)(id, SEL, id, uint64_t))objc_msgSend)([cls alloc], sel_registerName("initWithWindows:spaceID:"), (id)windows, target);
        if (!operation) { CFRelease(windows); CFRelease(number); return 5; }
        (void)perform(operation); // Opaque token; membership is the success criterion.
        [operation release];
        CFRelease(windows);
        CFRelease(number);
        return 0;
    }
}

// One window of `pid` by verified CGWindowID, retained; NULL when absent.
static AXUIElementRef copy_ax_window(int32_t pid, uint32_t wid) {
    pthread_once(&his_once, open_his);
    AXError (*get_window)(AXUIElementRef, uint32_t *) = his_handle ? dlsym(his_handle, "_AXUIElementGetWindow") : NULL;
    if (!get_window || pid <= 0 || !wid) return NULL;
    AXUIElementRef app = AXUIElementCreateApplication((pid_t)pid);
    if (!app) return NULL;
    AXUIElementRef found = NULL;
    CFArrayRef windows = NULL;
    if (AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, (CFTypeRef *)&windows) == kAXErrorSuccess && windows) {
        for (CFIndex i = 0; i < CFArrayGetCount(windows) && !found; i++) {
            AXUIElementRef window = (AXUIElementRef)CFArrayGetValueAtIndex(windows, i);
            uint32_t id = 0;
            if (get_window(window, &id) == kAXErrorSuccess && id == wid) found = (AXUIElementRef)CFRetain(window);
        }
    }
    if (windows) CFRelease(windows);
    CFRelease(app);
    return found;
}

// Native fullscreen on or off through AXFullScreen.
// Returns AXError, or -2 when the window is not found.
int tylora_set_window_fullscreen(int32_t pid, uint32_t wid, int fullscreen) {
    AXUIElementRef window = copy_ax_window(pid, wid);
    if (!window) return -2;
    AXError error = AXUIElementSetAttributeValue(window, CFSTR("AXFullScreen"), fullscreen ? kCFBooleanTrue : kCFBooleanFalse);
    CFRelease(window);
    return error;
}

// Position and size in global top-left coordinates.
// Returns AXError, or -2 when the window is not found.
int tylora_set_window_frame(int32_t pid, uint32_t wid, double x, double y, double width, double height) {
    AXUIElementRef window = copy_ax_window(pid, wid);
    if (!window) return -2;
    CGPoint origin = CGPointMake(x, y);
    CGSize size = CGSizeMake(width, height);
    AXValueRef position = AXValueCreate(kAXValueCGPointType, &origin);
    AXValueRef extent = AXValueCreate(kAXValueCGSizeType, &size);
    AXError error = position && extent ? kAXErrorSuccess : kAXErrorFailure;
    if (error == kAXErrorSuccess) error = AXUIElementSetAttributeValue(window, kAXPositionAttribute, position);
    if (error == kAXErrorSuccess) error = AXUIElementSetAttributeValue(window, kAXSizeAttribute, extent);
    if (position) CFRelease(position);
    if (extent) CFRelease(extent);
    CFRelease(window);
    return error;
}
