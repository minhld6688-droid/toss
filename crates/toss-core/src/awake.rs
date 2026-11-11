//! Keeps the machine awake while files move, and always on a host that must stay
//! reachable (the studio machine).
//!
//! One dedicated thread owns the OS request: Windows ties `SetThreadExecutionState`
//! to the calling thread, macOS uses an IOKit assertion, other systems do nothing.
//! The display may still turn off; only idle system sleep is held back.

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc, Mutex, OnceLock,
};

static COUNT: AtomicUsize = AtomicUsize::new(0);
static ALWAYS: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

/// Held for the length of a transfer session; dropping the last one lets the
/// machine sleep again (unless [`set_always`] is on).
#[must_use = "the machine may sleep as soon as the hold is dropped"]
pub struct Hold(());

impl Drop for Hold {
    fn drop(&mut self) {
        COUNT.fetch_sub(1, Ordering::SeqCst);
        poke();
    }
}

/// Keeps the machine awake until the returned value is dropped.
pub fn hold() -> Hold {
    COUNT.fetch_add(1, Ordering::SeqCst);
    poke();
    Hold(())
}

/// A host that must stay reachable: awake even with nothing moving.
pub fn set_always(on: bool) {
    ALWAYS.store(on, Ordering::SeqCst);
    poke();
}

/// True while something keeps the machine awake.
pub fn is_held() -> bool {
    ALWAYS.load(Ordering::SeqCst) || COUNT.load(Ordering::SeqCst) > 0
}

/// Wakes the owner thread, which reads the current state itself (so messages
/// arriving out of order can never leave a stale request behind).
fn poke() {
    let tx = WAKE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        let _ = std::thread::Builder::new()
            .name("toss-awake".into())
            .spawn(move || {
                let mut req = sys::Request::default();
                while rx.recv().is_ok() {
                    while rx.try_recv().is_ok() {}
                    req.set(is_held());
                }
                req.set(false);
            });
        Mutex::new(tx)
    });
    if let Ok(tx) = tx.lock() {
        let _ = tx.send(());
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_void};

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const LEVEL_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, enc: u32) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    #[derive(Default)]
    pub struct Request {
        id: Option<u32>,
    }

    impl Request {
        pub fn set(&mut self, on: bool) {
            match (on, self.id) {
                (true, None) => unsafe {
                    let kind = CFStringCreateWithCString(
                        std::ptr::null(),
                        c"PreventUserIdleSystemSleep".as_ptr(),
                        UTF8,
                    );
                    let name = CFStringCreateWithCString(std::ptr::null(), c"Toss: đang chuyển file".as_ptr(), UTF8);
                    let mut id = 0u32;
                    if !kind.is_null() && !name.is_null() && IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id) == 0 {
                        self.id = Some(id);
                    }
                    if !kind.is_null() {
                        CFRelease(kind);
                    }
                    if !name.is_null() {
                        CFRelease(name);
                    }
                },
                (false, Some(id)) => unsafe {
                    IOPMAssertionRelease(id);
                    self.id = None;
                },
                _ => {}
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    #[derive(Default)]
    pub struct Request {
        on: bool,
    }

    impl Request {
        pub fn set(&mut self, on: bool) {
            if on != self.on {
                let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
                unsafe { SetThreadExecutionState(flags) };
                self.on = on;
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod sys {
    /// Nothing to ask the OS for; the state is only remembered.
    #[derive(Default)]
    pub struct Request {
        _on: bool,
    }

    impl Request {
        pub fn set(&mut self, on: bool) {
            self._on = on;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_count_and_release() {
        let before = COUNT.load(Ordering::SeqCst);
        let a = hold();
        let b = hold();
        assert!(is_held());
        drop(a);
        assert!(is_held());
        drop(b);
        assert_eq!(COUNT.load(Ordering::SeqCst), before);
    }
}
