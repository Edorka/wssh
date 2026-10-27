pub mod service;
pub mod sessions;
extern crate clap;

use clap::{value_parser, Arg, ArgAction, Command};
use service::WSServer;
use sessions::sshclient::SSHClient;
use sessions::Sessions;
use std::env;
use std::time::Duration;

// Configuration struct to hold configurable values
#[derive(Debug)]
struct Config {
    ssh_port: u16,
    listen_address: String,
    target_host: String,
}

impl Config {
    fn from_env_and_args() -> Self {
        // Default values
        let mut config = Config {
            ssh_port: 22,
            listen_address: "127.0.0.1".to_string(),
            target_host: "127.0.0.1".to_string(),
        };

        // Override with environment variables if set
        if let Ok(ssh_port) = env::var("SSH_PORT") {
            if let Ok(port) = ssh_port.parse::<u16>() {
                config.ssh_port = port;
            }
        }

        if let Ok(listen_address) = env::var("LISTEN_ADDRESS") {
            config.listen_address = listen_address;
        }

        if let Ok(target_host) = env::var("TARGET_HOST") {
            config.target_host = target_host;
        }

        config
    }
}

const DEFAULT_TARGET_HOST: &str = "127.0.0.1";

fn main() {
    env_logger::init();
    
    let config = Config::from_env_and_args();

    let matches = Command::new("WebSocket to SSH (WSSH)")
        .version("0.1")
        .author("Eduardo Orive")
        .about("Creates connections to an SSH host")
        .arg(
            Arg::new("PORT")
                .short('p')
                .long("port")
                .help("listening HTTP port (default: 80)")
                .value_parser(value_parser!(u16))
                .default_value("80"),
        )
        .arg(
            Arg::new("ADDRESS")
                .short('a')
                .long("address")
                .help("Set target (SSH) host address")
                .default_value(DEFAULT_TARGET_HOST),
        )
        .arg(Arg::new("v").short('v').help("Sets the level of verbosity"))
        .arg(
            Arg::new("DEBUG")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("print debug information verbosely"),
        )
        .arg(
            Arg::new("TOKEN_ROTATION_PERIOD")
                .short('t')
                .long("token-rotation-period")
                .help("Session token rotation period in seconds (0 disables rotation)")
                .value_parser(value_parser!(u64))
                .default_value("300"),
        )
        .get_matches();

    let port = matches
        .get_one::<u16>("PORT")
        .expect("default ensures there is always a value");

    let address = matches
        .get_one::<String>("ADDRESS")
        .expect("default ensures there is always a value");

    let debug = matches.get_flag("DEBUG");

    let rotation_period = matches
        .get_one::<u64>("TOKEN_ROTATION_PERIOD")
        .expect("default ensures there is always a value");
    let rotation_period = match rotation_period {
        0 => None,
        secs => Some(Duration::from_secs(*secs)),
    };

    // Use the address from command line arguments, but fall back to config if needed
    let target_endpoint = format!("{}:{}", address, config.ssh_port);
    let provider = SSHClient {};
    let sessions = Sessions::new(&target_endpoint, provider, true, rotation_period);
    let listening_endpoint = format!("{}:{}", config.listen_address, port);
    let server = WSServer::new(&listening_endpoint, &sessions, debug);
    server.run();
}
