//! Common assertion macros and helpers for fshell test suites.

#[macro_export]
macro_rules! assert_val_eq {
    ($left:expr, $right:expr $(,)?) => {
        match (&$left, &$right) {
            (left_val, right_val) => {
                if *left_val != *right_val {
                    panic!(
                        "assertion failed: `(left == right)`\n  left: `{:?}`\n right: `{:?}`",
                        left_val, right_val
                    );
                }
            }
        }
    };
}

#[macro_export]
macro_rules! assert_val_int {
    ($val:expr, $expected:expr) => {
        match $val {
            $crate::common::Val::Int(n) => assert_eq!(n, $expected),
            other => panic!("expected Val::Int({}), got {:?}", $expected, other),
        }
    };
}

#[macro_export]
macro_rules! assert_val_str {
    ($val:expr, $expected:expr) => {
        match $val {
            $crate::common::Val::String(ref s) => assert_eq!(s.as_str(), $expected),
            other => panic!("expected Val::String({:?}), got {:?}", $expected, other),
        }
    };
}

#[macro_export]
macro_rules! assert_val_bool {
    ($val:expr, $expected:expr) => {
        match $val {
            $crate::common::Val::Bool(b) => assert_eq!(b, $expected),
            other => panic!("expected Val::Bool({}), got {:?}", $expected, other),
        }
    };
}
