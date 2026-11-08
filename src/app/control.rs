//! The one message a child can send back to uncl: "toggle the float".
//!
//! uncl does not nest. A second `uncl` started in the owner shell should do what
//! the `Home` key does — raise the float, or put it away — and a second one started
//! *inside* the float must say so and exit instead of drawing a terminal in a
//! terminal. Both answers belong to the instance that is already running, so the
//! new process has to reach it.
//!
//! There is no socket, no lock file and no signal here: the inner process writes
//! one private `OSC` to its stdout and exits. That stdout *is* the pane's pty,
//! and uncl is the only reader of it, so the sequence cannot escape into the
//! user's terminal and nothing has to be set up or cleaned up. `vt100` hands
//! every `OSC` it does not model to [`crate::app::term::callbacks`], which turns
//! this one into a [`Control`] request; the event loop drains it like a key
//! press.
//!
//! Which of the two answers a launch gets is decided by
//! [`crate::app::term::caps::ChildRole`], read from `UNCL_ROLE`: a child of the
//! owner pane asks for the toggle, a child of the float refuses to start. Only
//! the owner's parser is given a [`Control`], so even a child that emitted these
//! bytes by hand could not move a float that belongs to someone else.

use std::sync::atomic::{AtomicBool, Ordering};

/// `OSC uncl ; toggle ST` — "raise the float, or put it away".
///
/// A non-numeric `OSC` parameter is deliberate. Every numeric one is claimed by
/// something (`0`/`2` titles, `52` clipboard, `1337` iTerm2, …) and this is not
/// a request any terminal would understand anyway: only uncl reads it.
pub const TOGGLE_REQUEST: &[u8] = b"\x1b]uncl;toggle\x1b\\";

/// Does this `OSC`, already split into parameters, ask for the toggle?
pub fn is_toggle_request(params: &[&[u8]]) -> bool {
    match params {
        [name, verb] => *name == b"uncl".as_slice() && *verb == b"toggle".as_slice(),
        _ => false,
    }
}

/// Send the request to the uncl that owns this pty.
///
/// Used by the launching process (see `main::launch`): it is a single write to
/// stdout, so it works before any terminal state has been touched and costs
/// nothing when it fails.
pub fn send_toggle_request() -> std::io::Result<()> {
    use std::io::Write;

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(TOGGLE_REQUEST)?;
    stdout.flush()
}

/// A pending toggle, set on the reader thread and drained by the event loop.
///
/// It is an `AtomicBool` rather than a channel because the setter is the parser
/// callback, which must never block (a blocking reader is a frozen pane) and
/// must never fail. Two launches in the same frame are one toggle: the thing
/// being flipped is a boolean, so applying it twice is a no-op anyway.
#[derive(Debug, Default)]
pub struct Control {
    toggle: AtomicBool,
}

impl Control {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called from the parser: never blocks, never fails, cannot deadlock.
    pub fn request_toggle(&self) {
        self.toggle.store(true, Ordering::Release);
    }

    /// Called by the event loop once per frame: was there a request?
    pub fn take_toggle(&self) -> bool {
        self.toggle.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_a_private_osc() {
        // `ESC ]`, a name no terminal claims, one parameter, `ST`.
        assert_eq!(TOGGLE_REQUEST, b"\x1b]uncl;toggle\x1b\\".as_slice());
        assert!(TOGGLE_REQUEST.starts_with(b"\x1b]"));
        assert!(TOGGLE_REQUEST.ends_with(b"\x1b\\"));
    }

    #[test]
    fn only_the_exact_parameters_are_a_toggle() {
        assert!(is_toggle_request(&[&b"uncl"[..], &b"toggle"[..]]));
        assert!(!is_toggle_request(&[&b"uncl"[..]]));
        assert!(!is_toggle_request(&[&b"uncl"[..], &b"toggle"[..], &b""[..]]));
        assert!(!is_toggle_request(&[&b"uncl"[..], &b"Toggle"[..]]));
        assert!(!is_toggle_request(&[&b"2"[..], &b"toggle"[..]]));
        assert!(!is_toggle_request(&[]));
    }

    #[test]
    fn a_request_is_taken_exactly_once() {
        let control = Control::new();
        assert!(!control.take_toggle(), "nothing to take yet");

        control.request_toggle();
        control.request_toggle();
        assert!(control.take_toggle());
        assert!(!control.take_toggle(), "a second launch in the same frame is one toggle");
    }
}
