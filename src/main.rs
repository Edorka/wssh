pub mod service;
pub mod sessions;
extern crate clap;

use clap::{value_parser, Arg, ArgAction, Command};
use service::WSServer;
use sessions::sshclient::SSHClient;
use sessions::Sessions;

fn main() {
    let matches = Command::new("WebSocket to SSH (WSSH)")
        .version("0.1")
        .author("Bequant (www.bequant.com)")
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
                .help("Set target (SSH) host addres")
                .default_value("127.0.0.1"),
        )
        .arg(Arg::new("v").short('v').help("Sets the level of verbosity"))
        .arg(
            Arg::new("DEBUG")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("print debug information verbosely"),
        )
        .get_matches();

    let port = matches
        .get_one::<u16>("PORT")
        .expect("default ensures there is always a value");

    let address = matches
        .get_one::<String>("ADDRESS")
        .expect("default ensures there is always a value");

    let debug = matches.get_flag("DEBUG");

    let target_endpoint = format!("{address}:22");
    let provider = SSHClient {};
    let sessions = Sessions::new(&target_endpoint, provider, true);
    let listening_endpoint = format!("127.0.0.1:{}", port);
    let server = WSServer::new(&listening_endpoint, &sessions, debug);
    server.run();
}
