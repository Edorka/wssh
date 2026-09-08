extern crate websocket;

use crate::sessions::{attend_message, attend_session_message, Session, Sessions};
use std::net::TcpStream;
use std::thread;
use websocket::receiver::Reader;
use websocket::sender::Writer;
use websocket::server::NoTlsAcceptor;
use websocket::sync::{Client, Server};
use websocket::{Message, OwnedMessage};

pub struct WSServer<'a> {
    address: String,
    sessions: &'a Sessions,
    debug: bool,
}

impl<'a> WSServer<'a> {
    pub fn new(address: &str, sessions: &'a Sessions, debug: bool) -> Self {
        Self {
            address: address.to_string(),
            sessions: sessions,
            debug,
        }
    }
    fn fetch_message(
        &self,
        receiver: &mut Reader<TcpStream>,
        sender: &mut Writer<TcpStream>,
    ) -> Option<OwnedMessage> {
        let mut reception = receiver.incoming_messages();
        match reception.next() {
            Some(Ok(message)) => Some(message),
            Some(Err(e)) => {
                if self.debug {
                    println!("Error processing: {:?}", e);
                }
                let _ = sender.send_message(&Message::close());
                None
            }
            None => None,
        }
    }
    fn attend_messages(
        &self,
        receiver: &mut Reader<TcpStream>,
        sender: &mut Writer<TcpStream>,
    ) -> Option<Session> {
        let outcome = loop {
            match self.fetch_message(receiver, sender) {
                None => break None,
                Some(OwnedMessage::Text(input)) => {
                    let (response, outcome) = attend_message(input.to_string(), self.sessions);
                    sender
                        .send_message(&OwnedMessage::Text(response.to_string()))
                        .unwrap();
                    match outcome {
                        None => response,
                        Some(session) => {
                            break Some(session);
                        }
                    };
                }
                Some(OwnedMessage::Ping(data)) => {
                    sender.send_message(&OwnedMessage::Pong(data)).unwrap();
                }
                _ => (),
            }
        };
        outcome
    }
    fn attend_session_messages(
        &self,
        receiver: &mut Reader<TcpStream>,
        sender: &mut Writer<TcpStream>,
        session: Session,
    ) -> () {
        loop {
            match self.fetch_message(receiver, sender) {
                None => break,
                Some(OwnedMessage::Text(input)) => {
                    let response = attend_session_message(input.to_string(), &session);
                    sender
                        .send_message(&OwnedMessage::Text(response.to_string()))
                        .unwrap();
                }
                Some(OwnedMessage::Ping(data)) => {
                    sender.send_message(&OwnedMessage::Pong(data)).unwrap();
                }
                _ => (),
            }
        }
    }

    fn handle_connection(&self, connection: Client<TcpStream>) -> () {
        let (mut receiver, mut sender) = connection.split().unwrap();
        if let Some(session) = self.attend_messages(&mut receiver, &mut sender) {
            self.attend_session_messages(&mut receiver, &mut sender, session);
        }
    }

    pub fn accept_one(&self, instance: &mut Server<NoTlsAcceptor>) -> Option<Client<TcpStream>> {
        instance
            .filter_map(Result::ok)
            .next()
            .map(|upgrade| upgrade.accept().unwrap())
    }

    pub fn bind(&self) -> Server<NoTlsAcceptor> {
        let address = &self.address;
        let instance = Server::bind(address).unwrap();
        println!("Websocket Server listening on: {address}");
        instance
    }

    pub fn serve(&self, instance: Server<NoTlsAcceptor>) -> () {
        thread::scope(|s| {
            for connection in instance.filter_map(Result::ok) {
                let client = connection.accept().unwrap();
                s.spawn(move || {
                    println!("connected.");
                    self.handle_connection(client);
                });
            }
        });
    }

    pub fn run(&self) -> () {
        self.serve(self.bind());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::protocol::{Response, SeqResponse};
    use crate::sessions::sshclient::{RunOutput, RunResult, ShellProvider};
    use mockall::mock;
    use mockall::predicate::*;
    use ssh2::Session as Shell;
    use std::net::SocketAddr;
    use websocket::client::ClientBuilder;

    mock! {
        ShellProvider {}
        impl ShellProvider for ShellProvider {
            fn connect(&self, host: &str, user: &str, password: &str) -> Result<Shell, RunOutput>;
            fn run_command(&self, shell: &Shell, command: &str) -> RunResult;
        }
    }

    fn connect(address: SocketAddr) -> Client<TcpStream> {
        let url = format!("ws://{address}");
        ClientBuilder::new(&url)
            .unwrap()
            .connect_insecure()
            .unwrap()
    }

    #[test]
    fn test_ping_pong() {
        let sessions = Sessions::new("127.0.0.1:0", MockShellProvider::new(), true);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            client.send_message(&OwnedMessage::Ping(vec![])).unwrap();
            client
                .recv_message()
                .map(|reply| assert_eq!(reply, OwnedMessage::Pong(vec![])))
                .unwrap();
            let _ = client.shutdown();
            eprintln!("CLIENT DONE");
        });
    }

    #[test]
    fn test_login_flow() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));
        let sessions = Sessions::new("127.0.0.1:0", provider, true);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            client
                .send_message(&OwnedMessage::Text(
                    serde_json::json!({
                        "id": 1,
                        "msg": { "Login": { "user": "user", "passwd": "password" } }
                    })
                    .to_string(),
                ))
                .unwrap();
            let reply = client.recv_message().unwrap();
            match reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 1);
                    match response.msg {
                        Response::Authentication {
                            code, token, ..
                        } => {
                            assert_eq!(code, 0);
                            assert!(token.is_some());
                        }
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }
            let entries = sessions.entries.lock().unwrap();
            assert_eq!(entries.keys().len(), 1);
            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_login_flow_failed() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("wrong"))
            .returning(|_, _, _| Err(RunOutput("Authentication failed".to_string(), -18)));
        let sessions = Sessions::new("127.0.0.1:0", provider, true);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            client
                .send_message(&OwnedMessage::Text(
                    serde_json::json!({
                        "id": 2,
                        "msg": { "Login": { "user": "user", "passwd": "wrong" } }
                    })
                    .to_string(),
                ))
                .unwrap();
            let reply = client.recv_message().unwrap();
            match reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    match response.msg {
                        Response::Authentication {
                            code, token, ..
                        } => {
                            assert_eq!(code, -18);
                            assert!(token.is_none());
                        }
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }
            let entries = sessions.entries.lock().unwrap();
            assert_eq!(entries.keys().len(), 0);
            let _ = client.shutdown();
        });
    }
}
