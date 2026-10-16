// SPDX-License-Identifier: MPL-2.0

//! Linux `dm_split_args` tokenization for target parameter strings.
//!
//! Ordinary tokens borrow the ioctl input. Escaped tokens own a bounded scratch
//! copy that is wiped on drop, including parse failures in secure table loads.

use alloc::vec::Vec;
use core::ops::Deref;

use ostd::mm::VmWriter;

/// One decoded argument; escaped copies never survive their parsing scope.
pub(super) enum Argument<'a> {
    Borrowed(&'a str),
    Escaped {
        bytes: Vec<u8>,
        #[cfg(ktest)]
        wipe_observer: Option<fn(&[u8])>,
    },
}

impl Deref for Argument<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            Self::Borrowed(value) => value,
            // Removing ASCII backslashes cannot invalidate a UTF-8 token.
            Self::Escaped { bytes, .. } => core::str::from_utf8(bytes).unwrap(),
        }
    }
}

impl Drop for Argument<'_> {
    fn drop(&mut self) {
        if let Self::Escaped {
            bytes,
            #[cfg(ktest)]
            wipe_observer,
        } = self
        {
            let len = bytes.len();
            VmWriter::from(bytes.as_mut_slice()).fill_zeros(len);
            #[cfg(ktest)]
            if let Some(observer) = wipe_observer {
                observer(bytes);
            }
        }
    }
}

/// Splits ASCII whitespace while allowing backslash to quote any following byte.
/// A final backslash is literal, as in Linux `dm_split_args`.
pub(super) struct Arguments<'a> {
    remaining: &'a str,
}

impl<'a> Arguments<'a> {
    pub(super) fn new(params: &'a str) -> Self {
        Self { remaining: params }
    }
}

/// Matches Linux `isspace`, including vertical tab.
fn is_space(byte: u8) -> bool {
    byte == b' ' || (b'\t'..=b'\r').contains(&byte)
}

impl<'a> Iterator for Arguments<'a> {
    type Item = Argument<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let input = self
            .remaining
            .trim_start_matches(|ch: char| ch.is_ascii() && is_space(ch as u8));
        if input.is_empty() {
            return None;
        }
        let bytes = input.as_bytes();
        let mut end = 0;
        let mut escaped = false;
        while end < bytes.len() {
            if bytes[end] == b'\\' && end + 1 < bytes.len() {
                escaped = true;
                end += 2;
            } else if is_space(bytes[end]) {
                break;
            } else {
                end += 1;
            }
        }
        self.remaining = &input[end..];
        if !escaped {
            return Some(Argument::Borrowed(&input[..end]));
        }
        // Reserve once so no old allocation containing parameters is abandoned
        // during growth. The resulting token is no longer than the input.
        let mut decoded = Vec::with_capacity(end);
        let mut cursor = 0;
        while cursor < end {
            if bytes[cursor] == b'\\' && cursor + 1 < end {
                cursor += 1;
            }
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
        Some(Argument::Escaped {
            bytes: decoded,
            #[cfg(ktest)]
            wipe_observer: None,
        })
    }
}

#[cfg(ktest)]
mod tests {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn escaped_arguments_are_wiped_on_success_and_parse_failure() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        static WIPES: AtomicUsize = AtomicUsize::new(0);
        fn observe(bytes: &[u8]) {
            assert!(!bytes.is_empty());
            assert!(bytes.iter().all(|byte| *byte == 0));
            WIPES.fetch_add(1, Ordering::Relaxed);
        }
        fn parse(params: &str) -> Result<(), ()> {
            let mut args = Arguments::new(params);
            let mut path = args.next().ok_or(())?;
            let Argument::Escaped { wipe_observer, .. } = &mut path else {
                panic!("expected an owned escaped argument");
            };
            *wipe_observer = Some(observe);
            assert_eq!(&*path, "/dev/a b");
            let offset = args.next().ok_or(())?;
            offset.parse::<u64>().map_err(|_| ())?;
            Ok(())
        }
        let before = WIPES.load(Ordering::Relaxed);
        assert_eq!(parse("/dev/a\\ b 0"), Ok(()));
        assert_eq!(WIPES.load(Ordering::Relaxed), before + 1);
        assert_eq!(parse("/dev/a\\ b invalid"), Err(()));
        assert_eq!(WIPES.load(Ordering::Relaxed), before + 2);
        assert_eq!(parse("/dev/a\\ b"), Err(()));
        assert_eq!(WIPES.load(Ordering::Relaxed), before + 3);
    }

    #[ktest]
    fn decodes_linux_quoted_arguments() {
        let mut args = Arguments::new("  /dev/a\\ b\t8\\ 0  a\\\\b trail\\");
        assert_eq!(&*args.next().unwrap(), "/dev/a b");
        assert_eq!(&*args.next().unwrap(), "8 0");
        assert_eq!(&*args.next().unwrap(), "a\\b");
        assert_eq!(&*args.next().unwrap(), "trail\\");
        assert!(args.next().is_none());
        assert_eq!(&*Arguments::new("设备\\ 名").next().unwrap(), "设备 名");
        let mut args = Arguments::new("\x0b\x0cfirst\x0bsecond\r\n");
        assert_eq!(&*args.next().unwrap(), "first");
        assert_eq!(&*args.next().unwrap(), "second");
        assert!(args.next().is_none());
        assert!(args.next().is_none());
    }
}
