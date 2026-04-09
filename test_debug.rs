use lttw::log;

fn main() {
    // Initialize tracing with debug enabled
    let result = log::init_tracing_subscriber("./lttw.log".to_string(), true);
    println!("Trace init result: {:?}", result);
    
    // Try to log something
    tracing::debug!("Test debug message 1");
    tracing::debug!("Test debug message 2");
    
    // Flush
    std::thread::sleep(std::time::Duration::from_millis(100));
    println!("Done logging");
}
