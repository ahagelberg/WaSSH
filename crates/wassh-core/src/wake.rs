//! Waking the window from a background thread.
//!
//! Transports and probes run on their own threads and hand their results to the frame
//! loop through channels. Without a wake the window would only notice them on its next
//! scheduled frame, so the thread that produced something calls [`notify`] and the shell
//! paints at once instead of polling hard.

use std::sync::{Arc, OnceLock};

/// Asks the window for a frame; installed by the shell on its first frame.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

static WAKER: OnceLock<Waker> = OnceLock::new();

/// Install the window's waker. Later calls are ignored: one window, one waker.
pub fn install(waker: Waker) {
    let _ = WAKER.set(waker);
}

/// Whether a window has installed its waker yet.
pub fn installed() -> bool {
    WAKER.get().is_some()
}

/// Ask for a frame. A no-op until a window exists, which is what tests see.
pub fn notify() {
    if let Some(waker) = WAKER.get() {
        waker();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_reaches_the_installed_waker() {
        let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = woken.clone();
        install(Arc::new(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)));
        notify();
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst));
    }
}
