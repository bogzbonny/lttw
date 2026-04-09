/// The Logger is a utility intended to be used to write TUI debug information
/// to a separate file such that debug output may be read while the TUI is
/// running.
///
/// DebugFilepath is the path to the debug file. If this is empty, no debug
/// information will be written.
/// The debug filepath is specified at the top of the main file of the package
/// being debugged
use {once_cell::sync::Lazy, parking_lot::RwLock, std::fs::OpenOptions, std::io::prelude::*};

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

    tracing_subscriber::fmt()
        .with_writer(file)
        .with_target(false)
        .with_line_number(true)
        .with_file(true)
        .with_thread_names(false)
        .with_ansi(false)
        .init();

    // Mark tracing as initialized
    let _ = TRACING_INITIALIZED.set(());

    Ok(())
}

/// Get the current log file path (if set)
pub fn get_log_file() -> Option<String> {
    GLOBAL_LOGGER.read().log_file.clone()
}

/// Set the log file path
pub fn set_log_file(file: String) {
    (GLOBAL_LOGGER.write()).log_file = Some(file);
}

/// Reset the log file (clears and sets new path)
pub fn reset_log_file(file: String) {
    (GLOBAL_LOGGER.write()).log_file = Some(file.clone());
    clear();

    // Also try to initialize tracing subscriber for the new log file
    let debug_enabled = (GLOBAL_LOGGER.read()).enabled;
    let _ = init_tracing_subscriber(file.clone(), debug_enabled);
}

/// Clear the log file and ring buffer
pub fn clear() {
    (GLOBAL_LOGGER.write()).lines.clear();

    // clear file
    if let Some(file) = &(GLOBAL_LOGGER.write()).log_file {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(file)
            .expect("could not open log file");
        file.set_len(0).expect("could not truncate log file");
    }
}

/// Log content to the ring buffer and file
pub fn log(content: String) {
    if !GLOBAL_LOGGER.read().enabled {
        return;
    }

    let mut lines = GLOBAL_LOGGER.read().lines.clone();
    let max_lines = GLOBAL_LOGGER.read().max_lines;
    lines.push(content.clone());
    if lines.len() > max_lines {
        lines.remove(0);
    }
    (GLOBAL_LOGGER.write()).lines = lines;

    // push to file if configured
    if let Some(file) = GLOBAL_LOGGER.read().log_file.clone() {
        // Check if tracing is being used - if so, use tracing::debug instead
        // For the legacy log function, write directly to the file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .expect("could not open log file");
        writeln!(file, "{content}").expect("could not write to log file");
    }
}

/// Get content from the ring buffer
pub fn get_content() -> String {
    GLOBAL_LOGGER.read().lines.join("\n")
}

/// Get max lines for ring buffer
pub fn get_max_lines() -> usize {
    (GLOBAL_LOGGER.read()).max_lines
}

/// Check if logging is enabled
pub fn is_enabled() -> bool {
    (GLOBAL_LOGGER.write()).enabled
}

/// Enable logging
pub fn enable() {
    (GLOBAL_LOGGER.write()).enabled = true;
}

/// Disable logging
pub fn disable() {
    (GLOBAL_LOGGER.write()).enabled = false;
}

/// log or panic either logs the content or panics if the build mode is non-release
pub fn log_or_panic(content: String) {
    log(content.clone());
    #[cfg(debug_assertions)]
    panic!("{}", content);
}

#[derive(Clone)]
pub struct Logger {
    pub log_file: Option<String>, // if some then output to this file
    pub enabled: bool,
    pub max_lines: usize,
    pub lines: Vec<String>,
}

static GLOBAL_LOGGER: Lazy<RwLock<Logger>> = Lazy::new(|| {
    RwLock::new(Logger {
        log_file: None,
        enabled: true,
        max_lines: 300,
        lines: Vec::new(),
    })
});

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
