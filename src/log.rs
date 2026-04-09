/// The Logger is a utility intended to be used to write TUI debug information
/// to a separate file such that debug output may be read while the TUI is
/// running.
///
/// DebugFilepath is the path to the debug file. If this is empty, no debug
/// information will be written.
/// The debug filepath is specified at the top of the main file of the package
/// being debugged
use {
    std::fs::OpenOptions, tracing_subscriber::filter::LevelFilter,
    tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt, tracing_subscriber::Layer,
};

// Track if tracing has been initialized to avoid duplicate initialization
static TRACING_INITIALIZED: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();

/// Initialize the tracing subscriber with file output
/// If debug_enabled is false, tracing will not be initialized
/// Returns Ok(()) if tracing was initialized or already initialized
/// Returns Err if the file could not be opened
pub fn init_tracing_subscriber(
    file_path: String,
    debug_enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !debug_enabled {
        // Don't initialize tracing if debug is disabled
        return Ok(());
    }

    // Try to acquire the initialization cell - if it succeeds, we initialize tracing
    // If it fails, tracing is already initialized
    if TRACING_INITIALIZED.get().is_some() {
        // Tracing is already initialized, skip
        return Ok(());
    }

    // Use tracing_subscriber with a file writer
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)?;

    // Build the file writer layer for debug logging
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_target(false)
        .with_line_number(true)
        .with_file(true)
        .with_thread_names(false)
        .with_ansi(false)
        .with_filter(LevelFilter::DEBUG);

    // Build the subscriber with file layer and max level DEBUG
    let subscriber = tracing_subscriber::Registry::default()
        .with(file_layer)
        .with(LevelFilter::DEBUG);

    // Set as global default - this might fail if already set (e.g., in tests)
    let _ = tracing::subscriber::set_global_default(subscriber);

    // Mark tracing as initialized
    let _ = TRACING_INITIALIZED.set(());

    Ok(())
}

/// Reset the log file (clears and sets new path)
pub fn reset_log_file(file: String) {
    let _ = init_tracing_subscriber(file.clone(), true);
}

// Re-export tracing macros for use throughout the codebase
// This macro provides backward compatibility with the old debug! macro that could
// accept a single variable like debug!(var), which would print "var = value"
#[macro_export]
macro_rules! debug {
    // Single expression (variable) - like the old debug!(var) syntax
    // This expands to a format string with debug formatting
    ($expr:expr) => {{
        tracing::debug!(target: "lttw", "{} = {:?}", stringify!($expr), $expr);
    }};
    // Format-style variadic - like the old debug!("message {}", arg) syntax
    ($($arg:tt)*) => {{
        tracing::debug!($($arg)*);
    }};
}
