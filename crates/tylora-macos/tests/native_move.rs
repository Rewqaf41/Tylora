//! Real macOS integration test. Run only with a disposable ordinary window
//! chosen for this test and explicit source/target Space IDs.

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a user-selected disposable window and Accessibility permission"]
fn moves_one_window_and_restores_its_original_space() {
    use tylora_macos::spaces::{NativeSpaceId, NativeWindowSpaceMover};
    let parse = |key: &str| -> u64 {
        std::env::var(key)
            .unwrap_or_else(|_| panic!("set {key} explicitly"))
            .parse()
            .unwrap_or_else(|_| panic!("{key} must be an integer"))
    };
    let wid = u32::try_from(parse("TYLORA_TEST_WINDOW_ID")).expect("CGWindowID fits u32");
    let source = NativeSpaceId(parse("TYLORA_TEST_SOURCE_SPACE_ID"));
    let target = NativeSpaceId(parse("TYLORA_TEST_TARGET_SPACE_ID"));
    assert_ne!(source, target);
    assert_eq!(
        NativeWindowSpaceMover::can_move_window(wid).unwrap(),
        source
    );

    struct Restore {
        wid: u32,
        source: NativeSpaceId,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            use tylora_macos::spaces::NativeWindowSpaceMover;
            if NativeWindowSpaceMover::verify_window_space(self.wid, self.source) != Ok(true) {
                let _ = NativeWindowSpaceMover::move_window_to_space(self.wid, self.source);
            }
        }
    }
    let restore = Restore { wid, source };
    let moved = NativeWindowSpaceMover::move_window_to_space(wid, target).unwrap();
    assert!(moved.confirmed && !moved.no_op);
    assert_eq!(moved.source, source);
    assert_eq!(moved.target, target);
    assert_eq!(
        NativeWindowSpaceMover::verify_window_space(wid, target),
        Ok(true)
    );
    let returned = NativeWindowSpaceMover::move_window_to_space(wid, source).unwrap();
    assert!(returned.confirmed);
    assert_eq!(
        NativeWindowSpaceMover::verify_window_space(wid, source),
        Ok(true)
    );
    drop(restore);
}
