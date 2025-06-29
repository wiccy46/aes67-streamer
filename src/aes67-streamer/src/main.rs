use std::process;

fn main() {
    // Initialize logging
    env_logger::init();
    
    log::info!("Starting AES67 Audio Streamer");
    
    // Parse CLI arguments
    let args = match config::parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error parsing arguments: {}", e);
            process::exit(1);
        }
    };
    
    log::info!("Parsed arguments: {:?}", args);
    
    // TODO: Implement main streaming logic
    println!("AES67 Audio Streamer - Phase 1 Implementation");
    println!("Audio file: {}", args.file);
    println!("Multicast address: {}", args.address);
    println!("Port: {}", args.port);
    
    if let Some(interface) = &args.interface {
        println!("Network interface: {}", interface);
    }
}