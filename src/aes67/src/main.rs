use std::env;
use std::ffi::OsString;
use std::process;

const VERSION: &str = env!("AES67_TOOLS_VERSION");

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Help(HelpTopic),
    Version,
    Dispatch(Invocation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpTopic {
    Root,
    Send,
    SendFile,
    SendQueue,
    Receive,
    ReceiveDiscover,
    ReceiveListen,
    ReceiveDevices,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workflow {
    SendFile,
    SendQueue,
    ReceiveDiscover,
    ReceiveListen,
    ReceiveDevices,
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    workflow: Workflow,
    args: Vec<OsString>,
}

#[tokio::main]
async fn main() {
    let exit_code = match run(env::args_os().skip(1).collect()).await {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("Error: {error}");
            1
        }
    };

    process::exit(exit_code);
}

async fn run(args: Vec<OsString>) -> Result<i32, String> {
    match route(&args)? {
        Route::Help(topic) => {
            print_help(topic);
            Ok(0)
        }
        Route::Version => {
            println!("aes67 {}", version());
            Ok(0)
        }
        Route::Dispatch(invocation) => dispatch(invocation).await,
    }
}

fn route(args: &[OsString]) -> Result<Route, String> {
    let Some(command) = args.first() else {
        return Ok(Route::Help(HelpTopic::Root));
    };

    match command_name(command)? {
        "-h" | "--help" => Ok(Route::Help(HelpTopic::Root)),
        "-V" | "--version" => Ok(Route::Version),
        "send" => route_send(&args[1..]),
        "receive" => route_receive(&args[1..]),
        other => Err(unknown_command("aes67", other)),
    }
}

fn route_send(args: &[OsString]) -> Result<Route, String> {
    let Some(command) = args.first() else {
        return Ok(Route::Help(HelpTopic::Send));
    };

    match command_name(command)? {
        "-h" | "--help" => Ok(Route::Help(HelpTopic::Send)),
        "file" => route_leaf(HelpTopic::SendFile, Workflow::SendFile, args[1..].to_vec()),
        "queue" => route_leaf(
            HelpTopic::SendQueue,
            Workflow::SendQueue,
            args[1..].to_vec(),
        ),
        other => Err(unknown_command("aes67 send", other)),
    }
}

fn route_receive(args: &[OsString]) -> Result<Route, String> {
    let Some(command) = args.first() else {
        return Ok(Route::Help(HelpTopic::Receive));
    };

    match command_name(command)? {
        "-h" | "--help" => Ok(Route::Help(HelpTopic::Receive)),
        "discover" => route_leaf(
            HelpTopic::ReceiveDiscover,
            Workflow::ReceiveDiscover,
            args[1..].to_vec(),
        ),
        "listen" => route_leaf(
            HelpTopic::ReceiveListen,
            Workflow::ReceiveListen,
            args[1..].to_vec(),
        ),
        "devices" => route_devices(&args[1..]),
        other => Err(unknown_command("aes67 receive", other)),
    }
}

fn route_devices(args: &[OsString]) -> Result<Route, String> {
    if has_help(args) {
        return Ok(Route::Help(HelpTopic::ReceiveDevices));
    }
    if is_version_request(args) {
        return Ok(Route::Version);
    }
    if !args.is_empty() {
        return Err("aes67 receive devices does not accept options".to_string());
    }

    Ok(Route::Dispatch(Invocation {
        workflow: Workflow::ReceiveDevices,
        args: Vec::new(),
    }))
}

fn route_leaf(topic: HelpTopic, workflow: Workflow, args: Vec<OsString>) -> Result<Route, String> {
    if has_help(&args) {
        return Ok(Route::Help(topic));
    }
    if is_version_request(&args) {
        return Ok(Route::Version);
    }

    Ok(Route::Dispatch(Invocation { workflow, args }))
}

fn has_help(args: &[OsString]) -> bool {
    args.iter()
        .any(|arg| matches!(arg.to_str(), Some("-h" | "--help")))
}

fn is_version_request(args: &[OsString]) -> bool {
    matches!(args, [arg] if matches!(arg.to_str(), Some("-V" | "--version")))
}

fn command_name(command: &OsString) -> Result<&str, String> {
    command
        .to_str()
        .ok_or_else(|| "command names must be valid UTF-8".to_string())
}

fn unknown_command(parent: &str, command: &str) -> String {
    format!("unknown command {command:?} for {parent}; run `{parent} --help`")
}

async fn dispatch(invocation: Invocation) -> Result<i32, String> {
    match invocation.workflow {
        Workflow::SendFile => {
            let args = parse_send_file_args(invocation.args)?;
            initialise_logging(args.verbose);
            aes67_engine::sender::send_file(args)
                .await
                .map_err(|error| format!("send failed: {error:#}"))?;
        }
        Workflow::SendQueue => {
            if !invocation.args.is_empty() {
                return Err("aes67 send queue does not accept options".to_string());
            }
            initialise_logging(false);
            aes67_engine::sender::queue::run()
                .await
                .map_err(|error| format!("send queue failed: {error:#}"))?;
        }
        Workflow::ReceiveDiscover => {
            let args = parse_receive_discover_args(invocation.args)?;
            initialise_logging(args.verbose);
            aes67_engine::discovery::discover(args)
                .await
                .map_err(|error| format!("receive discovery failed: {error:#}"))?;
        }
        Workflow::ReceiveListen => {
            let args = parse_receive_listen_args(invocation.args)?;
            initialise_logging(args.verbose);
            aes67_engine::receiver::listen(args)
                .await
                .map_err(|error| format!("receive failed: {error:#}"))?;
        }
        Workflow::ReceiveDevices => {
            print!(
                "{}",
                aes67_engine::receiver::list_output_devices()
                    .map_err(|error| format!("failed to list audio devices: {error:#}"))?
            );
        }
    }

    Ok(0)
}

fn parse_send_file_args(args: Vec<OsString>) -> Result<config::StreamerArgs, String> {
    config::parse_send_file_args_from(with_program("aes67 send file", args))
        .map_err(|error| format!("Error parsing send arguments: {error}"))
}

fn parse_receive_listen_args(args: Vec<OsString>) -> Result<config::PlayerArgs, String> {
    config::parse_receive_listen_args_from(with_program("aes67 receive listen", args))
        .map_err(|error| format!("Error parsing receive arguments: {error}"))
}

fn parse_receive_discover_args(args: Vec<OsString>) -> Result<config::SapArgs, String> {
    config::parse_receive_discover_args_from(with_program("aes67 receive discover", args))
        .map_err(|error| format!("Error parsing discovery arguments: {error}"))
}

fn with_program(program: &str, args: Vec<OsString>) -> Vec<OsString> {
    let mut parsed_args = Vec::with_capacity(args.len() + 1);
    parsed_args.push(OsString::from(program));
    parsed_args.extend(args);
    parsed_args
}

fn initialise_logging(verbose: bool) {
    let default_level = if verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .init();
}

fn version() -> &'static str {
    VERSION.trim()
}

fn print_help(topic: HelpTopic) {
    let help = match topic {
        HelpTopic::Root => ROOT_HELP,
        HelpTopic::Send => SEND_HELP,
        HelpTopic::SendFile => SEND_FILE_HELP,
        HelpTopic::SendQueue => SEND_QUEUE_HELP,
        HelpTopic::Receive => RECEIVE_HELP,
        HelpTopic::ReceiveDiscover => RECEIVE_DISCOVER_HELP,
        HelpTopic::ReceiveListen => RECEIVE_LISTEN_HELP,
        HelpTopic::ReceiveDevices => RECEIVE_DEVICES_HELP,
    };
    print!("{help}");
}

const ROOT_HELP: &str = "AES67 Tools\n\n\
Usage: aes67 <COMMAND>\n\n\
Commands:\n\
  send       Put an audio source on an AES67 network\n\
  receive    Discover, join, and listen to one AES67 stream\n\n\
Run `aes67 <COMMAND> --help` for details.\n";

const SEND_HELP: &str = "AES67 Send\n\n\
Usage: aes67 send <COMMAND>\n\n\
Commands:\n\
  file       Send one audio file as an AES67 RTP stream\n\
  queue      Open the interactive queued sender\n";

const SEND_FILE_HELP: &str = "AES67 Send File\n\n\
Usage: aes67 send file --file <FILE> --address <IP> [OPTIONS]\n\n\
Options:\n\
  -c, --config <FILE>\n\
  -f, --file <FILE>\n\
  -a, --address <IP>\n\
  -p, --port <PORT>\n\
  -i, --interface <INTERFACE>\n\
      --sdp-output <FILE>\n\
      --ptp-domain <DOMAIN>\n\
      --duration-seconds <SECONDS>\n\
      --loop\n\
  -v, --verbose\n";

const SEND_QUEUE_HELP: &str = "AES67 Send Queue\n\n\
Usage: aes67 send queue\n\n\
Opens the terminal UI for configuring a stream and queueing audio files to send.\n";

const RECEIVE_HELP: &str = "AES67 Receive\n\n\
Usage: aes67 receive <COMMAND>\n\n\
Commands:\n\
  discover   List SAP-announced AES67 streams\n\
  listen     Receive one stream and output it locally\n\
  devices    List available local audio output devices\n";

const RECEIVE_DISCOVER_HELP: &str = "AES67 Receive Discover\n\n\
Usage: aes67 receive discover --interface <INTERFACE> [OPTIONS]\n\n\
Options:\n\
  -i, --interface <INTERFACE>\n\
      --once\n\
      --sdp-output-dir <DIR>\n\
  -v, --verbose\n";

const RECEIVE_LISTEN_HELP: &str = "AES67 Receive Listen\n\n\
Usage: aes67 receive listen (--sdp <FILE> | --address <IP> --port <PORT>) [OPTIONS]\n\n\
Options:\n\
      --sdp <FILE>\n\
  -a, --address <IP>\n\
  -p, --port <PORT>\n\
  -i, --interface <INTERFACE>\n\
      --sender <IP>\n\
      --channels <COUNT>\n\
      --payload-type <PT>\n\
      --latency-ms <MS>\n\
  -o, --output-device <DEVICE>\n\
      --duration-seconds <SECONDS>\n\
  -v, --verbose\n";

const RECEIVE_DEVICES_HELP: &str = "AES67 Receive Devices\n\n\
Usage: aes67 receive devices\n\n\
Lists local audio output devices and exits.\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn route_from(values: &[&str]) -> Route {
        route(&values.iter().map(OsString::from).collect::<Vec<OsString>>())
            .expect("route should succeed")
    }

    #[test]
    fn routes_send_to_engine_workflows() {
        assert_eq!(
            route_from(&[
                "send",
                "file",
                "--file",
                "tone.wav",
                "--address",
                "239.1.1.1"
            ]),
            Route::Dispatch(Invocation {
                workflow: Workflow::SendFile,
                args: vec![
                    OsString::from("--file"),
                    OsString::from("tone.wav"),
                    OsString::from("--address"),
                    OsString::from("239.1.1.1"),
                ],
            })
        );
        assert_eq!(
            route_from(&["send", "queue"]),
            Route::Dispatch(Invocation {
                workflow: Workflow::SendQueue,
                args: Vec::new(),
            })
        );
    }

    #[test]
    fn routes_receive_to_engine_workflows() {
        assert_eq!(
            route_from(&["receive", "discover", "--interface", "en0"]),
            Route::Dispatch(Invocation {
                workflow: Workflow::ReceiveDiscover,
                args: vec![OsString::from("--interface"), OsString::from("en0")],
            })
        );
        assert_eq!(
            route_from(&["receive", "listen", "--sdp", "main.sdp"]),
            Route::Dispatch(Invocation {
                workflow: Workflow::ReceiveListen,
                args: vec![OsString::from("--sdp"), OsString::from("main.sdp")],
            })
        );
        assert_eq!(
            route_from(&["receive", "devices"]),
            Route::Dispatch(Invocation {
                workflow: Workflow::ReceiveDevices,
                args: Vec::new(),
            })
        );
    }

    #[test]
    fn help_uses_the_two_product_lines() {
        assert_eq!(
            route_from(&["receive", "listen", "--help"]),
            Route::Help(HelpTopic::ReceiveListen)
        );
        assert_eq!(route_from(&["send", "file", "--version"]), Route::Version);
        assert!(ROOT_HELP.contains("send"));
        assert!(ROOT_HELP.contains("receive"));
        assert!(!ROOT_HELP.contains("verify"));
    }

    #[test]
    fn invalid_commands_have_a_clear_error() {
        let error = route(&[OsString::from("receive"), OsString::from("play")])
            .expect_err("unknown command should fail");

        assert!(error.contains("unknown command"));
        assert!(error.contains("aes67 receive --help"));
    }
}
