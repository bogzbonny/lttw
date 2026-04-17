/// src/server_launch.rs - Automatic llama.cpp server management
///
/// Handles checking if the llama.cpp server is already running and launching
/// it if needed, based on the `auto_launch` and `auto_launch_command` config options.
use {
    crate::{config::LttwConfig, get_state},
    std::net::TcpStream,
    std::process::Command,
};

/// Extract just the port number from an endpoint URL.
///
/// For example:
/// - `http://127.0.0.1:8012/infill` -> `8012`
/// - `http://localhost:8080/v1/chat/completions` -> `8080`
fn extract_port(endpoint: &str) -> Option<String> {
    let host_port = extract_host_port(endpoint)?;
    host_port.rsplit_once(':').map(|(_, port)| port.to_string())
}

/// Find PIDs of processes listening on a given port using `lsof`.
///
/// Returns a vector of PIDs as strings.
fn find_pids_on_port(port: &str) -> Vec<String> {
    match Command::new("lsof")
        .args(["-i", &format!("tcp:{}", port), "-t"])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        Ok(output) => {
            info!(
                "lsof returned non-zero status for port {}: {}",
                port,
                String::from_utf8_lossy(&output.stderr)
            );
            Vec::new()
        }
        Err(e) => {
            info!(
                "Could not run lsof to find processes on port {}: {}",
                port, e
            );
            Vec::new()
        }
    }
}

/// Kill all processes listening on a given port.
///
/// Uses `lsof` to find PIDs, then sends SIGKILL to each.
fn kill_processes_on_port(port: &str) -> usize {
    let pids = find_pids_on_port(port);
    let mut killed = 0;

    for pid_str in &pids {
        if let Ok(pid) = pid_str.parse::<u32>() {
            match Command::new("kill").arg("-9").arg(pid.to_string()).output() {
                Ok(output) => {
                    if !output.status.success() {
                        info!(
                            "Failed to kill PID {} on port {}: {}",
                            pid,
                            port,
                            String::from_utf8_lossy(&output.stderr)
                        );
                    } else {
                        killed += 1;
                    }
                }
                Err(e) => {
                    info!("Failed to run kill for PID {} on port {}: {}", pid, port, e);
                }
            }
        }
    }

    killed
}

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
/// Stores the process handle in PluginState for restart/stop functionality.
///
/// Returns `Ok(())` on success, or `Err` if spawning fails.
pub fn launch_server(config: &LttwConfig) -> Result<(), std::io::Error> {
    let command = &config.auto_launch_command;

    info!("Launching llama.cpp server with command: {}", command);

    // Use sh -c to parse the full shell command (handles nohup, redirects, etc.)
    match Command::new("sh").arg("-c").arg(command).spawn() {
        Ok(child) => {
            let pid = child.id();
            info!("llama.cpp server process spawned with PID: {}", pid);

            // Store the process handle in state for restart/stop functionality
            let state = get_state();
            *state.server_process.write() = Some(child);

            Ok(())
        }
        Err(e) => {
            error!("Failed to launch llama.cpp server: {}", e);
            Err(e)
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
    let _ = launch_server(config);
}

/// Stop the llama.cpp server process if it's currently running.
///
/// First tries to kill the stored Child handle. If no handle is stored
/// (e.g., server was already running when the plugin started), falls back
/// to finding and killing processes listening on the server's port using
/// `lsof` + `kill -9`.
pub fn stop_server() {
    let state = get_state();
    let mut process_lock = state.server_process.write();

    if let Some(ref mut child) = *process_lock {
        match child.kill() {
            Ok(()) => {
                info!("llama.cpp server process killed");
            }
            Err(e) => {
                error!("Failed to kill llama.cpp server process: {}", e);
            }
        }
        // Wait for the process to fully terminate
        let _ = child.wait();
        *process_lock = None;
    } else {
        // No stored handle — server may have been running before plugin startup.
        // Fall back to finding and killing by port.
        let config = state.config.read();
        let endpoint = config.get_endpoint(crate::fim::FimLLM::Fast);

        if let Some(port) = extract_port(&endpoint) {
            let killed = kill_processes_on_port(&port);
            if killed > 0 {
                info!(
                    "Killed {} process{} on port {}",
                    killed,
                    if killed > 1 { "es" } else { "" },
                    port
                );
            } else {
                info!("No llama.cpp server process found on port {}", port);
            }
        } else {
            info!("Could not extract port from endpoint to stop server");
        }
    }
}

/// Restart the llama.cpp server by stopping the current one and launching a new one.
///
/// If auto_launch is disabled in config, this is a no-op.
/// If the server is not currently running, this just launches a new one.
pub fn restart_server(config: &LttwConfig) {
    info!("Restarting llama.cpp server");

    // Stop the current server if running
    stop_server();

    // Small delay to allow the port to be released
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Launch the new server
    match launch_server(config) {
        Ok(_) => {
            info!("llama.cpp server restarted successfully");
        }
        Err(e) => {
            error!("Failed to restart llama.cpp server: {}", e);
        }
    }
}
