use ssh2::{Channel, ErrorCode, KeyboardInteractivePrompt, Prompt, Session};
use std::io::Read;
use std::marker::{Send, Sync};
use std::net::TcpStream; // In order to use Channel::read_to_string()

#[derive(Debug)]
pub struct RunOutput(pub String, pub i32);

pub enum RunResult {
    Ok(RunOutput),
    Err(RunOutput),
}

pub trait ShellProvider: Send + Sync {
    fn connect(&self, host: &str, user: &str, password: &str) -> Result<Session, RunOutput>;
    fn run_command(&self, shell: &Session, command: &str) -> RunResult;
}

#[derive(Debug)]
pub struct SSHClient;

impl SSHClient {
    pub fn new() -> SSHClient {
        SSHClient {}
    }
}

unsafe impl Send for SSHClient {}
unsafe impl Sync for SSHClient {}

impl ShellProvider for SSHClient {
    fn connect(&self, host: &str, username: &str, password: &str) -> Result<Session, RunOutput> {
        let tcp = TcpStream::connect(host.to_string()).unwrap();
        let mut sess = Session::new().unwrap();
        sess.set_tcp_stream(tcp);
        sess.handshake().unwrap();
        struct Prompter {
            password: String,
        }

        impl KeyboardInteractivePrompt for Prompter {
            fn prompt<'a>(
                &mut self,
                _username: &str,
                _instructions: &str,
                prompts: &[Prompt<'a>],
            ) -> Vec<String> {
                prompts.iter().map(|_| self.password.to_owned()).collect()
            }
        }

        let mut prompt = Prompter {
            password: password.to_string(),
        };
        match sess.userauth_keyboard_interactive(username, &mut prompt) {
            Ok(_) => Ok(sess),
            Err(ssh_error) => {
                let msg = ssh_error.message();
                match ssh_error.code() {
                    ErrorCode::Session(code_number) => Err(RunOutput(msg.to_string(), code_number)),
                    _ => Err(RunOutput(msg.to_string(), -255)),
                }
            }
        }
    }

    fn run_command(&self, shell: &Session, command: &str) -> RunResult {
        let mut channel: Channel = shell.channel_session().unwrap();
        channel.exec(command).unwrap();
        let mut s = String::new();
        channel.read_to_string(&mut s).unwrap();
        channel.wait_close().unwrap();
        match channel.exit_status() {
            Ok(code) => RunResult::Ok(RunOutput(s, code)),
            Err(ssh_error) => {
                let msg = ssh_error.message();
                match ssh_error.code() {
                    ErrorCode::Session(code_number) => {
                        RunResult::Err(RunOutput(msg.to_string(), code_number))
                    }
                    _ => RunResult::Err(RunOutput(msg.to_string(), -1)),
                }
            }
        }
    }
}
