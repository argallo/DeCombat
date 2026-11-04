// The Objective-C side of dc-ios-hid: GCKeyboard / GCMouse handlers feed a locked ring buffer that Rust drains once
// per frame (src/ios.rs mirrors struct dc_hid_event), and the pointer lock of the window's view controller.
//
// Threading: every handler runs on the device's handlerQueue, which GameController sets to the main queue by
// default; the ring is behind an os_unfair_lock anyway, so dc_hid_drain may be called from any thread.
//
// Pointer lock: UIKit asks the root view controller of the window's scene for `prefersPointerLocked` (following
// `childViewControllerForPointerLock`). winit 0.30's root controller is its own subclass (WinitUIViewController),
// which does not define the getter, so we add one to the concrete class of every window's root view controller
// (class_addMethod; if that class already defines it, its implementation is replaced). The getter returns a static
// flag. After a change `setNeedsUpdateOfPrefersPointerLocked` is sent to each root controller. Only that class is
// touched, not UIViewController itself.

#import <Foundation/Foundation.h>
#import <GameController/GameController.h>
#import <UIKit/UIKit.h>
#import <objc/runtime.h>
#import <os/lock.h>
#include <stdbool.h>
#include <stdio.h>
#include <stddef.h>
#include <stdint.h>

struct dc_hid_event {
    uint8_t kind;  // DC_HID_*
    uint8_t down;  // key / button: 1 down; presence: 1 connected
    uint16_t code; // key: USB HID usage id; button: 0 left, 1 right, 2 middle, 3.. auxiliary
    float dx;
    float dy;
};

enum {
    DC_HID_KEY = 1,
    DC_HID_MOVE = 2,
    DC_HID_BUTTON = 3,
    DC_HID_SCROLL = 4,
    DC_HID_KEYBOARD = 5,
    DC_HID_MOUSE = 6,
};

// The C API (src/ios.rs declares the same).
void dc_hid_start(void);
size_t dc_hid_drain(struct dc_hid_event *out, size_t cap);
void dc_hid_set_pointer_locked(bool on);
bool dc_hid_pointer_is_locked(void);
void dc_hid_relock(void);
void dc_hid_hide_pointer(bool hide);
void dc_hid_set_frame_rate(int max_hz);
int dc_hid_display_max_hz(void);
uint64_t dc_hid_link_ticks(void);

#define DC_HID_CAP 1024

static os_unfair_lock g_lock = OS_UNFAIR_LOCK_INIT;
static struct dc_hid_event g_ring[DC_HID_CAP];
static size_t g_head; // oldest queued event
static size_t g_len;

static void push(uint8_t kind, uint8_t down, uint16_t code, float dx, float dy) {
    struct dc_hid_event e = {kind, down, code, dx, dy};
    os_unfair_lock_lock(&g_lock);
    if (g_len == DC_HID_CAP) { // full: drop the oldest
        g_head = (g_head + 1) % DC_HID_CAP;
        g_len--;
    }
    g_ring[(g_head + g_len) % DC_HID_CAP] = e;
    g_len++;
    os_unfair_lock_unlock(&g_lock);
}

size_t dc_hid_drain(struct dc_hid_event *out, size_t cap) {
    if (out == NULL || cap == 0) {
        return 0;
    }
    os_unfair_lock_lock(&g_lock);
    size_t n = g_len < cap ? g_len : cap;
    for (size_t i = 0; i < n; i++) {
        out[i] = g_ring[(g_head + i) % DC_HID_CAP];
    }
    g_head = (g_head + n) % DC_HID_CAP;
    g_len -= n;
    os_unfair_lock_unlock(&g_lock);
    return n;
}

// ---- keyboard ------------------------------------------------------------------------------------------------

static bool g_keyboard_present;

static void attach_keyboard(GCKeyboard *keyboard) {
    GCKeyboardInput *input = keyboard.keyboardInput;
    if (input == nil) {
        return;
    }
    input.keyChangedHandler = ^(GCKeyboardInput *kb, GCDeviceButtonInput *key, GCKeyCode keyCode, BOOL pressed) {
        (void)kb;
        (void)key;
        if (keyCode < 0 || keyCode > 0xffff) {
            return;
        }
        push(DC_HID_KEY, pressed ? 1 : 0, (uint16_t)keyCode, 0, 0);
    };
    if (!g_keyboard_present) {
        g_keyboard_present = true;
        push(DC_HID_KEYBOARD, 1, 0, 0, 0);
    }
}

// ---- mouse ---------------------------------------------------------------------------------------------------

static NSMutableSet<GCMouse *> *g_mice;

static void attach_button(GCControllerButtonInput *button, uint16_t index) {
    if (button == nil) {
        return;
    }
    button.pressedChangedHandler = ^(GCControllerButtonInput *b, float value, BOOL pressed) {
        (void)b;
        (void)value;
        push(DC_HID_BUTTON, pressed ? 1 : 0, index, 0, 0);
    };
}

static void attach_mouse(GCMouse *mouse) {
    if (mouse == nil || [g_mice containsObject:mouse]) {
        return;
    }
    GCMouseInput *input = mouse.mouseInput;
    if (input == nil) {
        return;
    }
    input.mouseMovedHandler = ^(GCMouseInput *m, float deltaX, float deltaY) {
        (void)m;
        // GameController's deltaY grows upward (a Cartesian axis); the Rust side wants +y down.
        push(DC_HID_MOVE, 0, 0, deltaX, -deltaY);
    };
    attach_button(input.leftButton, 0);
    attach_button(input.rightButton, 1);
    attach_button(input.middleButton, 2);
    NSArray<GCDeviceButtonInput *> *aux = input.auxiliaryButtons;
    for (NSUInteger i = 0; i < aux.count && i < 250; i++) {
        attach_button(aux[i], (uint16_t)(3 + i));
    }
    input.scroll.valueChangedHandler = ^(GCControllerDirectionPad *pad, float x, float y) {
        (void)pad;
        // passed through as reported (taken as the motion of this event); an all-zero report carries nothing
        if (x != 0 || y != 0) {
            push(DC_HID_SCROLL, 0, 0, x, y);
        }
    };
    bool was_empty = g_mice.count == 0;
    [g_mice addObject:mouse];
    if (was_empty) {
        push(DC_HID_MOUSE, 1, 0, 0, 0);
    }
}

static void detach_mouse(GCMouse *mouse) {
    if (mouse == nil || ![g_mice containsObject:mouse]) {
        return;
    }
    [g_mice removeObject:mouse];
    if (g_mice.count == 0) {
        push(DC_HID_MOUSE, 0, 0, 0, 0);
    }
}

// ---- pointer lock --------------------------------------------------------------------------------------------

static volatile bool g_pointer_locked;

static BOOL dc_prefers_pointer_locked(id self, SEL _cmd) {
    (void)self;
    (void)_cmd;
    return g_pointer_locked ? YES : NO;
}

static NSMutableSet<Class> *g_patched; // classes whose prefersPointerLocked is ours

static void patch_class(Class cls) {
    if (cls == Nil || [g_patched containsObject:cls]) {
        return;
    }
    SEL sel = @selector(prefersPointerLocked);
    IMP imp = (IMP)dc_prefers_pointer_locked;
    if (!class_addMethod(cls, sel, imp, "B@:")) {
        // the class defines its own getter: replace it
        Method m = class_getInstanceMethod(cls, sel);
        if (m != NULL) {
            method_setImplementation(m, imp);
        }
    }
    [g_patched addObject:cls];
}

// Every window of every connected scene: patch its root controller's class and ask UIKit to re-read the flag.
static void apply_pointer_lock(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:[UIWindowScene class]]) {
            continue;
        }
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            UIViewController *root = window.rootViewController;
            if (root == nil) {
                continue;
            }
            patch_class(object_getClass(root));
            [root setNeedsUpdateOfPrefersPointerLocked];
        }
    }
}

// Whether any window scene reports the pointer locked right now (UIKit's own state; the system lets the lock go
// on its own at times).
bool dc_hid_pointer_is_locked(void) {
    static bool noted;
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if ([scene isKindOfClass:[UIWindowScene class]]) {
            UIPointerLockState *st = ((UIWindowScene *)scene).pointerLockState;
            if (!noted) {
                noted = true;
                fprintf(stderr, "dc-ios-hid: pointerLockState %s, locked %d\n", st == nil ? "nil" : "set",
                        st != nil && st.locked);
            }
            if (st != nil && st.locked) {
                return true;
            }
        }
    }
    return false;
}

// Asks for the lock again by toggling the flag: UIKit re-reads it only on a change.
void dc_hid_relock(void) {
    dispatch_block_t apply = ^{
        g_pointer_locked = false;
        apply_pointer_lock();
        g_pointer_locked = true;
        apply_pointer_lock();
    };
    if (NSThread.isMainThread) {
        apply();
    } else {
        dispatch_async(dispatch_get_main_queue(), apply);
    }
}

void dc_hid_set_pointer_locked(bool on) {
    g_pointer_locked = on;
    dispatch_block_t apply = ^{
        apply_pointer_lock();
    };
    if (NSThread.isMainThread) {
        apply();
    } else {
        dispatch_async(dispatch_get_main_queue(), apply);
    }
}

// ---- hidden pointer ------------------------------------------------------------------------------------------
// The system pointer hidden over the app's view through a UIPointerInteraction whose style is the hidden one (the
// pointer lock is not always granted: Stage Manager, a scene that is not the focused full-screen one); the mouse
// still reports through GameController and its clicks still arrive as touches.
@interface DCPointerHider : NSObject <UIPointerInteractionDelegate>
@end

@implementation DCPointerHider
- (UIPointerRegion *)pointerInteraction:(UIPointerInteraction *)interaction
                       regionForRequest:(UIPointerRegionRequest *)request
                          defaultRegion:(UIPointerRegion *)defaultRegion {
    (void)request;
    UIView *view = interaction.view;
    if (view == nil) {
        return defaultRegion;
    }
    return [UIPointerRegion regionWithRect:view.bounds identifier:@"decombat"];
}

- (UIPointerStyle *)pointerInteraction:(UIPointerInteraction *)interaction styleForRegion:(UIPointerRegion *)region {
    (void)interaction;
    (void)region;
    return [UIPointerStyle hiddenPointerStyle];
}
@end

static DCPointerHider *g_hider;
static UIPointerInteraction *g_hider_interaction;

static void hide_pointer_on_main(bool hide) {
    if (g_hider == nil) {
        g_hider = [DCPointerHider new];
    }
    UIView *view = nil;
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:[UIWindowScene class]]) {
            continue;
        }
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            if (window.rootViewController.view != nil) {
                view = window.rootViewController.view;
                break;
            }
        }
        if (view != nil) {
            break;
        }
    }
    if (view == nil) {
        return;
    }
    if (g_hider_interaction == nil || g_hider_interaction.view != view) {
        if (g_hider_interaction != nil && g_hider_interaction.view != nil) {
            [g_hider_interaction.view removeInteraction:g_hider_interaction];
        }
        g_hider_interaction = [[UIPointerInteraction alloc] initWithDelegate:g_hider];
        [view addInteraction:g_hider_interaction];
    }
    g_hider_interaction.enabled = hide;
    [g_hider_interaction invalidate];
}

void dc_hid_hide_pointer(bool hide) {
    if (NSThread.isMainThread) {
        hide_pointer_on_main(hide);
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            hide_pointer_on_main(hide);
        });
    }
}

// ---- frame rate ----------------------------------------------------------------------------------------------
// A ProMotion iPad runs an app at 60 Hz unless something in it asks for more: a CADisplayLink on the main run loop
// with a preferredFrameRateRange up to 120 tells the system the rate the Metal layer's presents may reach. The
// link's callback does nothing (the client draws from its own loop).
static CADisplayLink *g_link;
static id g_link_target;
static uint64_t g_link_ticks;

@interface DCLinkTarget : NSObject
- (void)tick:(CADisplayLink *)link;
@end

@implementation DCLinkTarget
- (void)tick:(CADisplayLink *)link {
    g_link_ticks++;
    if (g_link_ticks == 1 || g_link_ticks % 600 == 0) {
        CAFrameRateRange r = link.preferredFrameRateRange;
        fprintf(stderr, "dc-ios-hid: link firing %llu: interval %.2f ms (target - now %.2f ms), range %.0f..%.0f pref %.0f\n",
                (unsigned long long)g_link_ticks, link.duration * 1000.0,
                (link.targetTimestamp - link.timestamp) * 1000.0, r.minimum, r.maximum, r.preferred);
    }
}
@end

uint64_t dc_hid_link_ticks(void) {
    return g_link_ticks;
}

static void frame_rate_on_main(int max_hz) {
    if (max_hz <= 0) {
        if (g_link != nil) {
            [g_link invalidate];
            g_link = nil;
        }
        return;
    }
    float max = (float)max_hz;
    float min = max >= 100.0f ? 80.0f : (max >= 60.0f ? 60.0f : 30.0f);
    if (g_link == nil) {
        g_link_target = [DCLinkTarget new];
        // the link of the main screen (the display the app's window is on), its range set before it joins the
        // run loop
        g_link = [UIScreen.mainScreen displayLinkWithTarget:g_link_target selector:@selector(tick:)];
        g_link.preferredFrameRateRange = CAFrameRateRangeMake(min, max, max);
        [g_link addToRunLoop:NSRunLoop.mainRunLoop forMode:NSRunLoopCommonModes];
    } else {
        g_link.preferredFrameRateRange = CAFrameRateRangeMake(min, max, max);
    }
    fprintf(stderr, "dc-ios-hid: display max %ld Hz, link asks %d Hz\n",
            (long)UIScreen.mainScreen.maximumFramesPerSecond, max_hz);
}

int dc_hid_display_max_hz(void) {
    return (int)UIScreen.mainScreen.maximumFramesPerSecond;
}

void dc_hid_set_frame_rate(int max_hz) {
    if (NSThread.isMainThread) {
        frame_rate_on_main(max_hz);
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            frame_rate_on_main(max_hz);
        });
    }
}

// ---- start ---------------------------------------------------------------------------------------------------

static void start_on_main(void) {
    static bool started;
    if (started) {
        return;
    }
    started = true;
    g_mice = [NSMutableSet set];
    g_patched = [NSMutableSet set];

    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    NSOperationQueue *main = NSOperationQueue.mainQueue;
    [center addObserverForName:GCKeyboardDidConnectNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *note) {
                        GCKeyboard *keyboard = note.object;
                        if (![keyboard isKindOfClass:[GCKeyboard class]]) {
                            keyboard = GCKeyboard.coalescedKeyboard;
                        }
                        if (keyboard != nil) {
                            attach_keyboard(keyboard);
                        }
                    }];
    [center addObserverForName:GCKeyboardDidDisconnectNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *note) {
                        (void)note;
                        if (g_keyboard_present) {
                            g_keyboard_present = false;
                            push(DC_HID_KEYBOARD, 0, 0, 0, 0);
                        }
                    }];
    [center addObserverForName:GCMouseDidConnectNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *note) {
                        if ([note.object isKindOfClass:[GCMouse class]]) {
                            attach_mouse(note.object);
                        }
                    }];
    [center addObserverForName:GCMouseDidDisconnectNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *note) {
                        if ([note.object isKindOfClass:[GCMouse class]]) {
                            detach_mouse(note.object);
                        }
                    }];
    // A scene / window that appears later (or a controller winit swaps in) gets the lock state too.
    [center addObserverForName:UISceneDidActivateNotification
                        object:nil
                         queue:main
                    usingBlock:^(NSNotification *note) {
                        (void)note;
                        apply_pointer_lock();
                    }];

    GCKeyboard *keyboard = GCKeyboard.coalescedKeyboard;
    if (keyboard != nil) {
        attach_keyboard(keyboard);
    }
    for (GCMouse *mouse in GCMouse.mice) {
        attach_mouse(mouse);
    }
    // install the getter now, so the first lock request needs no class patching
    apply_pointer_lock();
}

void dc_hid_start(void) {
    if (NSThread.isMainThread) {
        start_on_main();
    } else {
        dispatch_async(dispatch_get_main_queue(), ^{
            start_on_main();
        });
    }
}
