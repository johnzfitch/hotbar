pub mod anim;
pub mod app;
pub mod config;
pub mod dispatch;
pub mod gpu;
pub mod keybinds;
pub mod sctk_shell;
pub mod theme;
pub mod widgets;

/// Trace-level span that compiles to nothing in release builds.
///
/// Usage: `dev_trace_span!("name")` or `dev_trace_span!("name", field = val)`
///
/// Declares a guard variable that keeps the span entered for the current scope.
/// In release builds, expands to nothing — zero overhead.
macro_rules! dev_trace_span {
    ($($args:tt)*) => {
        #[cfg(debug_assertions)]
        let _dev_span = tracing::trace_span!($($args)*).entered();
    };
}

pub(crate) use dev_trace_span;
