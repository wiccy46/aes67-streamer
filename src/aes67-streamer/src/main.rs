use std::process;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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

    log::info!("Audio file: {}", args.file);
    log::info!("Multicast address: {}", args.address);
    log::info!("Port: {}", args.port);

    if let Some(interface) = &args.interface {
        log::info!("Network interface: {}", interface);
    }

    // Initialize audio reader
    let _reader = match audio::AudioReader::new(&args.file) {
        Ok(reader) => {
            log::info!("Audio file loaded successfully");
            reader
        }
        Err(e) => {
            eprintln!("Failed to load audio file: {}", e);
            process::exit(1);
        }
    };

    // TODO: Implement streaming pipeline
    println!("Streaming pipeline not yet implemented - Phase 2 in progress");

    log::info!("AES67 streamer initialization complete")
}
