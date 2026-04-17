/// src/server_launch.rs - Automatic llama.cpp server management
///
/// Handles checking if the llama.cpp server is already running and launching
/// it if needed, based on the `auto_launch` and `auto_launch_command` config options.
use {crate::config::LttwConfig, std::net::TcpStream, std::process::Command};

/// Extract the host:port from a full endpoint URL.
///
/// For example:
/// - `http://127.0.0.1:8012/infill` -> `127.0.0.1:8012`
/// - `http://localhost:8080/v1/chat/completions` -> `localhost:8080`
fn extract_host_port(endpoint: &str) -> Option<String> {
    // Find the scheme separator "://"
    let scheme_end = endpoint.find("://")? + 3;

    // Find the first / after the scheme (this marks the start of the path)
    let path_start = endpoint[scheme_end..].find('/')? + scheme_end;

    // Extract host:port between scheme end and path start
    let host_port = &endpoint[scheme_end..path_start];
    if !host_port.is_empty() {
        Some(host_port.to_string())
    } else {
        None
    }
}

/// Check if the llama.cpp server is already running by attempting a TCP connection
/// to the port specified in the configured FIM endpoint.
///
/// Returns `true` if a connection can be established, `false` otherwise.
/// Uses a short timeout to avoid blocking startup for too long.
pub fn is_server_running(config: &LttwConfig) -> bool {
    let endpoint = config.get_endpoint(crate::fim::FimLLM::Fast);
    let host_port = match extract_host_port(&endpoint) {
        Some(addr) => addr,
        None => {
            info!("Could not extract host:port from endpoint: {}", endpoint);
            return false;
        }
    };

    // Parse host:port into a SocketAddr, then connect with timeout
    match host_port.parse::<std::net::SocketAddr>() {
        Ok(addr) => {
            TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
        }
        Err(_) => {
            info!(
                "Could not parse host:port as SocketAddr: {}, trying connect instead",
                host_port
            );
            // Fallback: use connect() without timeout for non-IP addresses
            TcpStream::connect(&host_port).is_ok()
        }
    }
}

/// Launch the llama.cpp server using the configured auto_launch_command.
///
/// Parses the command string and spawns it as a detached process using `sh -c`.
/// This is a synchronous operation that returns immediately after spawning.
pub fn launch_server(config: &LttwConfig) {
    let command = &config.auto_launch_command;

    info!("Launching llama.cpp server with command: {}", command);

    // Use sh -c to parse the full shell command (handles nohup, redirects, etc.)
    match Command::new("sh").arg("-c").arg(command).spawn() {
        Ok(child) => {
            info!("llama.cpp server process spawned with PID: {}", child.id());
        }
        Err(e) => {
            error!("Failed to launch llama.cpp server: {}", e);
        }
    }
}

/// Check if the server is running and optionally launch it.
///
/// Returns `true` if the server is available (either already running or successfully launched),
/// `false` if auto_launch is disabled or launching failed.
pub fn ensure_server_running(config: &LttwConfig) {
    if !config.auto_launch {
        info!("auto_launch is disabled, not attempting to start llama.cpp server");
        return;
    }

    if is_server_running(config) {
        info!("llama.cpp server is already running");
        return;
    }

    info!("llama.cpp server not running, attempting to launch...");
    launch_server(config);
}
