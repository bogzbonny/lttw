use lttw::debug;
use lttw::log;

fn main() {
    // Initialize tracing with debug enabled
    println!("Initializing tracing...");
    match log::init_tracing_subscriber("./lttw.log".to_string(), true) {
        Ok(()) => println!("Tracing initialized successfully"),
        Err(e) => eprintln!("Error initializing tracing: {}", e),
    }

    // Try to log something
    println!("Logging debug message...");
    debug!("This is a test debug message");
    debug!(123);
    debug!("Message with {}: {}", "arg", "value");

    // Force a flush
    std::thread::sleep(std::time::Duration::from_millis(100));

    println!("Done logging. Check lttw.log");
}
