//! Real-window experiment for the Animation Engine: how smoothly can another
//! application's window be moved and resized through AXPosition/AXSize?
//!
//! Run only against a disposable window that Tylora leaves unmanaged (a
//! floating panel, or an empty TextEdit document made floating):
//!
//! ```sh
//! cargo run -p tylora-macos --example animation_probe -- <pid> \
//!     [--dx 400] [--dy 0] [--dw 0] [--dh 0] [--ms 180] [--fps 60] [--eui-off]
//! ```
//!
//! It animates the first AX window of `pid` from its frame to the frame
//! shifted by the deltas, with ease-out-cubic timing on a monotonic clock,
//! and reports:
//! - the latency of every AX write,
//! - how often WindowServer actually showed a new frame (sampled from
//!   CGWindowListCopyWindowInfo on another thread every 2 ms), which is the
//!   real visual update rate, not the scheduler's rate,
//! - the final geometry error read back through AX.
//!
//! The original frame is restored at the end.

#[cfg(target_os = "macos")]
fn main() {
    probe::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}

#[cfg(target_os = "macos")]
mod probe {
    use std::{
        ffi::{c_char, c_void, CString},
        ptr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread,
        time::{Duration, Instant},
    };

    type CFRef = *const c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    const AX_POINT: u32 = 1;
    const AX_SIZE: u32 = 2;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> CFRef;
        fn AXUIElementCopyAttributeValue(
            element: CFRef,
            attribute: CFRef,
            value: *mut CFRef,
        ) -> i32;
        fn AXUIElementSetAttributeValue(element: CFRef, attribute: CFRef, value: CFRef) -> i32;
        fn AXValueCreate(kind: u32, value: *const c_void) -> CFRef;
        fn AXValueGetValue(value: CFRef, kind: u32, out: *mut c_void) -> u8;
        fn _AXUIElementGetWindow(element: CFRef, window: *mut u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: CFRef, value: *const c_char, encoding: u32) -> CFRef;
        fn CFArrayGetCount(array: CFRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
        fn CFDictionaryGetValue(dictionary: CFRef, key: CFRef) -> CFRef;
        fn CFRelease(value: CFRef);
        static kCFBooleanTrue: CFRef;
        static kCFBooleanFalse: CFRef;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFRef, rect: *mut Rect) -> u8;
        static kCGWindowBounds: CFRef;
    }

    fn cf_string(value: &str) -> CFRef {
        let value = CString::new(value).expect("attribute name");
        unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), 0x0800_0100) }
    }

    fn copy(element: CFRef, name: &str) -> Option<CFRef> {
        let key = cf_string(name);
        let mut value = ptr::null();
        let code = unsafe { AXUIElementCopyAttributeValue(element, key, &mut value) };
        unsafe { CFRelease(key) };
        (code == 0 && !value.is_null()).then_some(value)
    }

    fn frame(window: CFRef) -> Option<Rect> {
        let position = copy(window, "AXPosition")?;
        let size = copy(window, "AXSize")?;
        let mut rect = Rect::default();
        let ok = unsafe {
            AXValueGetValue(position, AX_POINT, (&mut rect.origin as *mut Point).cast()) != 0
                && AXValueGetValue(size, AX_SIZE, (&mut rect.size as *mut Size).cast()) != 0
        };
        unsafe {
            CFRelease(position);
            CFRelease(size);
        }
        ok.then_some(rect)
    }

    /// Writes one attribute and returns (AX error, latency).
    fn set(window: CFRef, name: &str, kind: u32, value: *const c_void) -> (i32, Duration) {
        let key = cf_string(name);
        let value = unsafe { AXValueCreate(kind, value) };
        let started = Instant::now();
        let code = unsafe { AXUIElementSetAttributeValue(window, key, value) };
        let elapsed = started.elapsed();
        unsafe {
            CFRelease(value);
            CFRelease(key);
        }
        (code, elapsed)
    }

    fn set_bool(element: CFRef, name: &str, value: bool) -> i32 {
        let key = cf_string(name);
        let value = unsafe {
            if value {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            }
        };
        let code = unsafe { AXUIElementSetAttributeValue(element, key, value) };
        unsafe { CFRelease(key) };
        code
    }

    /// The window's bounds as WindowServer shows them right now.
    fn cg_bounds(window_id: u32) -> Option<Rect> {
        // kCGWindowListOptionIncludingWindow
        let list = unsafe { CGWindowListCopyWindowInfo(8, window_id) };
        if list.is_null() {
            return None;
        }
        let mut rect = Rect::default();
        let found = unsafe { CFArrayGetCount(list) } > 0 && {
            let info = unsafe { CFArrayGetValueAtIndex(list, 0) };
            let bounds = unsafe { CFDictionaryGetValue(info, kCGWindowBounds) };
            !bounds.is_null()
                && unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) } != 0
        };
        unsafe { CFRelease(list) };
        found.then_some(rect)
    }

    fn ease_out_cubic(t: f64) -> f64 {
        1.0 - (1.0 - t).powi(3)
    }

    fn lerp(a: f64, b: f64, s: f64) -> f64 {
        a + (b - a) * s
    }

    struct Options {
        pid: i32,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
        ms: u64,
        fps: f64,
        eui_off: bool,
    }

    fn options() -> Options {
        let mut args = std::env::args().skip(1);
        let pid = args
            .next()
            .and_then(|pid| pid.parse().ok())
            .expect("usage: animation_probe <pid> [--dx N] [--dy N] [--dw N] [--dh N] [--ms N] [--fps N] [--eui-off]");
        let mut options = Options {
            pid,
            dx: 400.0,
            dy: 0.0,
            dw: 0.0,
            dh: 0.0,
            ms: 180,
            fps: 60.0,
            eui_off: false,
        };
        while let Some(flag) = args.next() {
            if flag == "--eui-off" {
                options.eui_off = true;
                continue;
            }
            let value: f64 = args
                .next()
                .and_then(|value| value.parse().ok())
                .expect("numeric flag value");
            match flag.as_str() {
                "--dx" => options.dx = value,
                "--dy" => options.dy = value,
                "--dw" => options.dw = value,
                "--dh" => options.dh = value,
                "--ms" => options.ms = value as u64,
                "--fps" => options.fps = value,
                other => panic!("unknown flag {other}"),
            }
        }
        options
    }

    fn stats(label: &str, samples: &mut [f64]) {
        if samples.is_empty() {
            println!("{label}: none");
            return;
        }
        samples.sort_by(f64::total_cmp);
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
        println!(
            "{label}: n={} mean={mean:.2} ms p95={p95:.2} ms max={:.2} ms",
            samples.len(),
            samples[samples.len() - 1]
        );
    }

    pub fn run() {
        let options = options();
        if unsafe { AXIsProcessTrusted() } == 0 {
            eprintln!("Accessibility permission is required");
            return;
        }
        let app = unsafe { AXUIElementCreateApplication(options.pid) };
        let windows = copy(app, "AXWindows").expect("AXWindows");
        assert!(unsafe { CFArrayGetCount(windows) } > 0, "no AX window");
        let window = unsafe { CFArrayGetValueAtIndex(windows, 0) };
        let mut window_id = 0;
        assert_eq!(unsafe { _AXUIElementGetWindow(window, &mut window_id) }, 0);
        let start = frame(window).expect("AX frame");
        let target = Rect {
            origin: Point {
                x: start.origin.x + options.dx,
                y: start.origin.y + options.dy,
            },
            size: Size {
                width: start.size.width + options.dw,
                height: start.size.height + options.dh,
            },
        };
        let resizing = options.dw != 0.0 || options.dh != 0.0;
        println!(
            "window {window_id}: {start:?} -> {target:?}, {} ms at {} fps{}",
            options.ms,
            options.fps,
            if options.eui_off {
                ", AXEnhancedUserInterface off"
            } else {
                ""
            }
        );
        // Restore only what was there: never turn the attribute on for an app
        // that did not have it.
        let eui_was_on = copy(app, "AXEnhancedUserInterface").is_some_and(|value| {
            let on = value == unsafe { kCFBooleanTrue };
            unsafe { CFRelease(value) };
            on
        });
        println!(
            "AXEnhancedUserInterface was {}",
            if eui_was_on { "on" } else { "off/absent" }
        );
        if options.eui_off && eui_was_on {
            println!(
                "disable AXEnhancedUserInterface: AX {}",
                set_bool(app, "AXEnhancedUserInterface", false)
            );
        }

        // Sample what WindowServer shows, independently of our writes.
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::<(Instant, Rect)>::new()));
        let sampler = {
            let (stop, seen) = (stop.clone(), seen.clone());
            thread::spawn(move || {
                let mut last = None;
                while !stop.load(Ordering::Acquire) {
                    if let Some(rect) = cg_bounds(window_id) {
                        if last != Some(rect) {
                            last = Some(rect);
                            seen.lock().unwrap().push((Instant::now(), rect));
                        }
                    }
                    thread::sleep(Duration::from_millis(2));
                }
            })
        };
        thread::sleep(Duration::from_millis(50));

        let duration = Duration::from_millis(options.ms);
        let interval = Duration::from_secs_f64(1.0 / options.fps);
        let began = Instant::now();
        let mut position_writes = Vec::new();
        let mut size_writes = Vec::new();
        let mut errors = 0;
        let mut frames = 0;
        let mut next = began;
        loop {
            // Time, not frame count, drives progress: a late frame catches up.
            let elapsed = began.elapsed();
            let t = (elapsed.as_secs_f64() / duration.as_secs_f64()).min(1.0);
            let s = ease_out_cubic(t);
            let rect = Rect {
                origin: Point {
                    x: lerp(start.origin.x, target.origin.x, s).round(),
                    y: lerp(start.origin.y, target.origin.y, s).round(),
                },
                size: Size {
                    width: lerp(start.size.width, target.size.width, s).round(),
                    height: lerp(start.size.height, target.size.height, s).round(),
                },
            };
            if resizing {
                let (code, latency) = set(
                    window,
                    "AXSize",
                    AX_SIZE,
                    (&rect.size as *const Size).cast(),
                );
                size_writes.push(latency.as_secs_f64() * 1000.0);
                errors += i32::from(code != 0);
            }
            let (code, latency) = set(
                window,
                "AXPosition",
                AX_POINT,
                (&rect.origin as *const Point).cast(),
            );
            position_writes.push(latency.as_secs_f64() * 1000.0);
            errors += i32::from(code != 0);
            frames += 1;
            if t >= 1.0 {
                break;
            }
            next += interval;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else {
                // Behind schedule: skip missed slots instead of bursting.
                next = now;
            }
        }
        let animated = began.elapsed();
        thread::sleep(Duration::from_millis(150));
        stop.store(true, Ordering::Release);
        sampler.join().unwrap();
        let landed = frame(window);

        println!(
            "scheduled frames: {frames} in {:.1} ms (AX errors: {errors})",
            animated.as_secs_f64() * 1000.0
        );
        stats("AXPosition write", &mut position_writes);
        if resizing {
            stats("AXSize write", &mut size_writes);
        }
        let seen = seen.lock().unwrap();
        let during: Vec<_> = seen
            .iter()
            .filter(|(at, _)| *at >= began && *at <= began + animated + Duration::from_millis(100))
            .collect();
        let mut gaps: Vec<f64> = during
            .windows(2)
            .map(|pair| (pair[1].0 - pair[0].0).as_secs_f64() * 1000.0)
            .collect();
        println!(
            "WindowServer frames seen: {} (effective {:.0} fps over the animation)",
            during.len(),
            during.len() as f64 / animated.as_secs_f64().max(1e-3)
        );
        stats("WindowServer frame interval", &mut gaps);
        if let Some(landed) = landed {
            let error = (landed.origin.x - target.origin.x).abs()
                + (landed.origin.y - target.origin.y).abs()
                + (landed.size.width - target.size.width).abs()
                + (landed.size.height - target.size.height).abs();
            println!("final frame {landed:?}, error {error:.1} pt");
        }

        thread::sleep(Duration::from_millis(300));
        let _ = set(
            window,
            "AXSize",
            AX_SIZE,
            (&start.size as *const Size).cast(),
        );
        let _ = set(
            window,
            "AXPosition",
            AX_POINT,
            (&start.origin as *const Point).cast(),
        );
        if options.eui_off && eui_was_on {
            let _ = set_bool(app, "AXEnhancedUserInterface", true);
        }
        println!("restored {:?}", frame(window));
        unsafe {
            CFRelease(windows);
            CFRelease(app);
        }
    }
}
