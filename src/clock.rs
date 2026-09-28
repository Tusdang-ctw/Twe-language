//! web3d-M2: a monotonic clock that works on every target.
//!
//! `std::time::Instant::now()` *panics* on `wasm32-unknown-unknown`, so
//! code that runs in the browser (the GC's sweep budget, profiling) must
//! not call it. Natively this is `Instant`; on wasm the host installs a
//! clock — `macroquad::time::get_time` in the 2D web build, the page's
//! `performance.now()` in the WebGPU shell — and until one is installed
//! [`now_secs`] returns `None`, so callers fall back to untimed
//! behaviour instead of crashing.

#[cfg(target_arch = "wasm32")]
thread_local! {
    static HOST_CLOCK: std::cell::Cell<Option<fn() -> f64>> = const { std::cell::Cell::new(None) };
}

/// Install the host's clock (seconds since an arbitrary fixed origin).
/// A no-op natively, where `Instant` is used.
pub fn install_host_clock(clock: fn() -> f64) {
    #[cfg(target_arch = "wasm32")]
    HOST_CLOCK.with(|c| c.set(Some(clock)));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = clock;
}

/// Seconds since an arbitrary fixed origin, or `None` when no clock is
/// available (wasm before the host installed one).
pub fn now_secs() -> Option<f64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let origin = *ORIGIN.get_or_init(std::time::Instant::now);
        Some(origin.elapsed().as_secs_f64())
    }
    #[cfg(target_arch = "wasm32")]
    {
        HOST_CLOCK.with(|c| c.get()).map(|f| f())
    }
}
