//! Real-window experiment for the Scrolling layout: how far outside the
//! visible display area does WindowServer/the application let an ordinary
//! window be positioned through AXPosition, and does AXSize survive?
//!
//! Run only against a disposable window (for example an empty TextEdit
//! document) that Tylora treats as floating:
//!
//! ```sh
//! cargo run -p tylora-macos --example offscreen_probe -- <pid> [x,y ...]
//! ```
//!
//! The probe moves the first AX window of `pid`, reads back the frame after
//! each request, and restores the original frame at the end.

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
        ptr, thread,
        time::Duration,
    };

    type CFRef = *const c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug)]
    struct Size {
        width: f64,
        height: f64,
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
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: CFRef);
        fn CFArrayGetCount(array: CFRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
        fn CFStringCreateWithCString(allocator: CFRef, text: *const c_char, encoding: u32)
            -> CFRef;
    }

    fn key(name: &str) -> CFRef {
        let name = CString::new(name).unwrap();
        unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100) }
    }

    fn read<T: Default>(window: CFRef, attribute: &str, kind: u32) -> Option<T> {
        let attr = key(attribute);
        let mut value = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(window, attr, &mut value) };
        unsafe { CFRelease(attr) };
        if error != 0 || value.is_null() {
            return None;
        }
        let mut out = T::default();
        let ok = unsafe { AXValueGetValue(value, kind, (&mut out as *mut T).cast()) } != 0;
        unsafe { CFRelease(value) };
        ok.then_some(out)
    }

    fn write<T>(window: CFRef, attribute: &str, kind: u32, value: &T) -> i32 {
        let attr = key(attribute);
        let boxed = unsafe { AXValueCreate(kind, (value as *const T).cast()) };
        let error = unsafe { AXUIElementSetAttributeValue(window, attr, boxed) };
        unsafe {
            CFRelease(boxed);
            CFRelease(attr);
        }
        error
    }

    pub fn run() {
        let mut args = std::env::args().skip(1);
        let pid: i32 = args
            .next()
            .and_then(|pid| pid.parse().ok())
            .expect("usage: offscreen_probe <pid> [x,y ...]");
        let mut requests: Vec<Point> = args
            .filter_map(|pair| {
                let (x, y) = pair.split_once(',')?;
                Some(Point {
                    x: x.parse().ok()?,
                    y: y.parse().ok()?,
                })
            })
            .collect();
        if unsafe { AXIsProcessTrusted() } == 0 {
            eprintln!("Accessibility permission is required");
            std::process::exit(2);
        }
        let app = unsafe { AXUIElementCreateApplication(pid) };
        let windows_key = key("AXWindows");
        let mut windows = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(app, windows_key, &mut windows) };
        if error != 0 || windows.is_null() || unsafe { CFArrayGetCount(windows) } == 0 {
            eprintln!("no AX window for pid {pid} (error {error})");
            std::process::exit(1);
        }
        let window = unsafe { CFArrayGetValueAtIndex(windows, 0) };
        let original_position: Point = read(window, "AXPosition", AX_POINT).unwrap();
        let original_size: Size = read(window, "AXSize", AX_SIZE).unwrap();
        println!("original position={original_position:?} size={original_size:?}");
        let size = Size {
            width: 600.0,
            height: 400.0,
        };
        println!("set size error={}", write(window, "AXSize", AX_SIZE, &size));
        if requests.is_empty() {
            requests = [
                (200.0, 200.0),
                (1100.0, 200.0),
                (1400.0, 200.0),
                (1520.0, 200.0),
                (3000.0, 200.0),
                (-300.0, 200.0),
                (-560.0, 200.0),
                (-700.0, 200.0),
                (-3000.0, 200.0),
                (200.0, 900.0),
                (200.0, 1200.0),
            ]
            .into_iter()
            .map(|(x, y)| Point { x, y })
            .collect();
        }
        for request in requests {
            let error = write(window, "AXPosition", AX_POINT, &request);
            thread::sleep(Duration::from_millis(250));
            let position: Option<Point> = read(window, "AXPosition", AX_POINT);
            let size: Option<Size> = read(window, "AXSize", AX_SIZE);
            println!(
                "request=({:.0},{:.0}) error={error} actual={:?} size={:?}",
                request.x,
                request.y,
                position.map(|p| (p.x, p.y)),
                size.map(|s| (s.width, s.height))
            );
        }
        write(window, "AXSize", AX_SIZE, &original_size);
        write(window, "AXPosition", AX_POINT, &original_position);
        unsafe {
            CFRelease(windows);
            CFRelease(windows_key);
            CFRelease(app);
        }
    }
}
