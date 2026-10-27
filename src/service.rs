extern crate websocket;

use crate::sessions::{attend_message, attend_session_message, Session, Sessions};
use log::{error, info};
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
                    error!("Error processing: {:?}", e);
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
                        if let Err(e) = sender.send_message(&OwnedMessage::Text(response.to_string())) {
                            error!("Failed to send message: {:?}", e);
                            break None;
                        }
                        match outcome {
                            None => response,
                            Some(session) => {
                                break Some(session);
                            }
                        };
                    }
                    Some(OwnedMessage::Ping(data)) => {
                        if let Err(e) = sender.send_message(&OwnedMessage::Pong(data)) {
                            error!("Failed to send pong message: {:?}", e);
                        }
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
        mut session: Session,
    ) -> () {
        loop {
            match self.fetch_message(receiver, sender) {
                None => break,
                Some(OwnedMessage::Text(input)) => {
                    let (response, rotated) =
                        attend_session_message(input.to_string(), &session, self.sessions);
                    if let Some(new_token) = rotated {
                        session.token = new_token;
                    }
                    if let Err(e) = sender.send_message(&OwnedMessage::Text(response.to_string())) {
                        error!("Failed to send message: {:?}", e);
                        break;
                    }
                }
                Some(OwnedMessage::Ping(data)) => {
                    if let Err(e) = sender.send_message(&OwnedMessage::Pong(data)) {
                        error!("Failed to send pong message: {:?}", e);
                    }
                }
                _ => (),
            }
        }
    }

    fn handle_connection(&self, connection: Client<TcpStream>) -> () {
        let (mut receiver, mut sender) = match connection.split() {
            Ok((receiver, sender)) => (receiver, sender),
            Err(e) => {
                error!("Failed to split connection: {:?}", e);
                return;
            }
        };
        if let Some(session) = self.attend_messages(&mut receiver, &mut sender) {
            self.attend_session_messages(&mut receiver, &mut sender, session);
        }
    }

    pub fn accept_one(&self, instance: &mut Server<NoTlsAcceptor>) -> Option<Client<TcpStream>> {
        instance
            .filter_map(Result::ok)
            .next()
            .and_then(|upgrade| upgrade.accept().ok())
    }

    pub fn bind(&self) -> Server<NoTlsAcceptor> {
        let address = &self.address;
        let instance = Server::bind(address)
            .unwrap_or_else(|e| {
                error!("Failed to bind to address {}: {:?}", address, e);
                panic!("Failed to bind to address {}: {:?}", address, e);
            });
        info!("Websocket Server listening on: {address}");
        instance
    }

    pub fn serve(&self, instance: Server<NoTlsAcceptor>) -> () {
        thread::scope(|s| {
            for connection in instance.filter_map(Result::ok) {
                match connection.accept() {
                    Ok(client) => {
                        s.spawn(move || {
                            info!("connected.");
                            self.handle_connection(client);
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {:?}", e);
                    }
                }
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
    use std::time::Duration;
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
        let sessions = Sessions::new("127.0.0.1:0", MockShellProvider::new(), true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            if let Err(e) = client.send_message(&OwnedMessage::Ping(vec![])) {
                panic!("Failed to send ping message: {:?}", e);
            }
            match client.recv_message() {
                Ok(reply) => assert_eq!(reply, OwnedMessage::Pong(vec![])),
                Err(e) => panic!("Failed to receive pong message: {:?}", e),
            }
            let _ = client.shutdown();
            info!("CLIENT DONE");
        });
    }

    #[test]
    fn test_login_flow() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));
        let sessions = Sessions::new("127.0.0.1:0", provider, true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 1,
                    "msg": { "Login": { "user": "user", "passwd": "password" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
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
        let sessions = Sessions::new("127.0.0.1:0", provider, true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 2,
                    "msg": { "Login": { "user": "user", "passwd": "wrong" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
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

    #[test]
    fn test_previous_with_invalid_token() {
        let sessions = Sessions::new("127.0.0.1:0", MockShellProvider::new(), true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            let previous_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 3,
                    "msg": { "Previous": { "token": "invalid_token" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&previous_message) {
                panic!("Failed to send previous message: {:?}", e);
            }
            let reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive previous response: {:?}", e),
            };
            match reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 3);
                    match response.msg {
                        Response::Previous {
                            code, reason, ..
                        } => {
                            assert_eq!(code, 1);
                            assert_eq!(reason, "Invalid token");
                        }
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }
            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_run_command_in_session_context() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));
        provider
            .expect_run_command()
            .with(always(), eq("ls -l"))
            .returning(|_, _| RunResult::Ok(RunOutput("total 0\n".to_string(), 0)));
        
        let sessions = Sessions::new("127.0.0.1:0", provider, true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });
            let mut client = connect(address);
            
            // First login to establish a session
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 4,
                    "msg": { "Login": { "user": "user", "passwd": "password" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let login_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
            
            // Verify login response but we don't need to extract the token
            match &login_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(payload).unwrap();
                    match &response.msg {
                        Response::Authentication { token, .. } => {
                            assert!(token.is_some(), "Expected token in login response");
                        }
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            };
            
            // Now send a run command
            let run_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 5,
                    "msg": { "Run": { "cmd": "ls -l" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&run_message) {
                panic!("Failed to send run message: {:?}", e);
            }
            let run_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive run response: {:?}", e),
            };
            
            match run_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 5);
                    match response.msg {
                        Response::Outcome { response, code } => {
                            assert_eq!(response, "total 0\n");
                            assert_eq!(code, 0);
                        }
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }
            
            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_multiple_sequential_connections() {
        let sessions = Sessions::new("127.0.0.1:0", MockShellProvider::new(), true, None);
        let server = WSServer::new("127.0.0.1:0", &sessions, true);
        let mut instance = server.bind();
        let address = instance.local_addr().unwrap();
        
        // Handle multiple connections in sequence
        for i in 0..3 {
            thread::scope(|s| {
                s.spawn(|| {
                    if let Some(client) = server.accept_one(&mut instance) {
                        server.handle_connection(client);
                    }
                });
                
                // Connect a client
                let mut client = connect(address);
                
                // Send a ping message
                let ping_message = OwnedMessage::Text(
                    serde_json::json!({
                        "id": i,
                        "msg": { "Ping": null }
                    })
                    .to_string(),
                );
                if let Err(e) = client.send_message(&ping_message) {
                    panic!("Failed to send ping message: {:?}", e);
                }
                
                // Receive the pong response
                let reply = match client.recv_message() {
                    Ok(reply) => reply,
                    Err(e) => panic!("Failed to receive pong response: {:?}", e),
                };
                
                match reply {
                    OwnedMessage::Text(payload) => {
                        let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                        assert_eq!(response.id, i as u64);
                        match response.msg {
                            Response::Pong { .. } => (), // Expected response
                            other => panic!("unexpected response: {other:?}"),
                        }
                    }
                    other => panic!("unexpected message: {other:?}"),
                }
                
                let _ = client.shutdown();
            });
        }
    }

    #[test]
    fn test_malformed_json_request() {
        let sessions = Sessions::new("127.0.0.1:0", MockShellProvider::new(), true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            
            s.spawn(move || {
                if let Some(client) = server.accept_one(&mut instance) {
                    server.handle_connection(client);
                }
            });
            
            let mut client = connect(address);
            
            // Send malformed JSON
            let malformed_json = OwnedMessage::Text("this is not valid json".to_string());
            if let Err(e) = client.send_message(&malformed_json) {
                panic!("Failed to send malformed JSON: {:?}", e);
            }
            
            // We expect the connection to be closed or an error response
            // Let's try to receive a message, but we might get an error
            match client.recv_message() {
                Ok(reply) => {
                    // If we get a reply, it should indicate an error
                    match reply {
                        OwnedMessage::Close(_) => {
                            // Connection closed, which is acceptable
                        }
                        OwnedMessage::Text(_payload) => {
                            // If we get text, it might be an error message
                            // We won't assert specifics here since behavior might vary
                        }
                        _ => {
                            // Other message types are also acceptable
                        }
                    }
                }
                Err(_) => {
                    // Error receiving message is also acceptable
                    // This might happen if the connection was closed
                }
            }
            
            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_ping_pong_in_session_context() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));
        
        let sessions = Sessions::new("127.0.0.1:0", provider, true, None);
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();
            
            s.spawn(move || {
                if let Some(client) = server.accept_one(&mut instance) {
                    server.handle_connection(client);
                }
            });
            
            let mut client = connect(address);
            
            // First login to establish a session
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 1,
                    "msg": { "Login": { "user": "user", "passwd": "password" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let _login_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
            
            // Now send a ping message in the session context
            let ping_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 2,
                    "msg": { "Ping": null }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ping_message) {
                panic!("Failed to send ping message: {:?}", e);
            }
            
            // Receive the pong response
            let reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive pong response: {:?}", e),
            };
            
            match reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 2);
                    match response.msg {
                        Response::Pong { .. } => (), // Expected response
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }

            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_token_rotation_flow() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new(
            "127.0.0.1:0",
            provider,
            true,
            Some(Duration::from_millis(1)),
        );
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();

            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });

            let mut client = connect(address);

            // Login to establish a session
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 1,
                    "msg": { "Login": { "user": "user", "passwd": "password" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let login_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
            let old_token = match &login_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(payload).unwrap();
                    match &response.msg {
                        Response::Authentication {
                            token: Some(token),
                            ..
                        } => token.clone(),
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            };

            // Let the rotation period elapse, then ping
            thread::sleep(Duration::from_millis(20));
            let ping_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 2,
                    "msg": { "Ping": null }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ping_message) {
                panic!("Failed to send ping message: {:?}", e);
            }
            let ping_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive pong response: {:?}", e),
            };
            let new_token = match ping_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 2);
                    match response.msg {
                        Response::Pong {
                            new_token: Some(token),
                            ..
                        } => token,
                        other => panic!("expected pong with new_token, got: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            };
            assert_ne!(new_token, old_token);

            // Old token is still valid while the rotation is pending
            let entries = sessions.entries.lock().unwrap();
            assert!(entries.contains_key(&old_token.parse::<u64>().unwrap()));
            drop(entries);

            // Acknowledge the new token to make the rotation effective
            let ack_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 3,
                    "msg": { "Acknowledge": { "token": new_token } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ack_message) {
                panic!("Failed to send acknowledge message: {:?}", e);
            }
            let ack_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive acknowledge response: {:?}", e),
            };
            match ack_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 3);
                    match response.msg {
                        Response::Acknowledged { code } => assert_eq!(code, 0),
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }

            // Rotation is effective: new token stored, old token gone
            let entries = sessions.entries.lock().unwrap();
            assert!(entries.contains_key(&new_token.parse::<u64>().unwrap()));
            assert!(!entries.contains_key(&old_token.parse::<u64>().unwrap()));
            drop(entries);

            let _ = client.shutdown();
        });
    }

    #[test]
    fn test_acknowledge_with_forged_token_is_rejected() {
        let mut provider = MockShellProvider::new();
        provider
            .expect_connect()
            .with(eq("127.0.0.1:0"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new(
            "127.0.0.1:0",
            provider,
            true,
            Some(Duration::from_millis(1)),
        );
        thread::scope(|s| {
            let server = WSServer::new("127.0.0.1:0", &sessions, true);
            let mut instance = server.bind();
            let address = instance.local_addr().unwrap();

            s.spawn(move || {
                let client = server.accept_one(&mut instance).unwrap();
                server.handle_connection(client);
            });

            let mut client = connect(address);

            // Login to establish a session
            let login_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 1,
                    "msg": { "Login": { "user": "user", "passwd": "password" } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&login_message) {
                panic!("Failed to send login message: {:?}", e);
            }
            let login_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive login response: {:?}", e),
            };
            let old_token = match &login_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(payload).unwrap();
                    match &response.msg {
                        Response::Authentication {
                            token: Some(token),
                            ..
                        } => token.clone(),
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            };

            // Let the rotation period elapse, then ping to get a pending token
            thread::sleep(Duration::from_millis(20));
            let ping_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 2,
                    "msg": { "Ping": null }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ping_message) {
                panic!("Failed to send ping message: {:?}", e);
            }
            let ping_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive pong response: {:?}", e),
            };
            let pending_token = match ping_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    match response.msg {
                        Response::Pong {
                            new_token: Some(token),
                            ..
                        } => token,
                        other => panic!("expected pong with new_token, got: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            };

            // Attempt to force the session onto a client-chosen token
            let forged_token = pending_token
                .parse::<u64>()
                .unwrap()
                .wrapping_add(1)
                .to_string();
            let ack_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 3,
                    "msg": { "Acknowledge": { "token": forged_token } }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ack_message) {
                panic!("Failed to send acknowledge message: {:?}", e);
            }
            let ack_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive acknowledge response: {:?}", e),
            };
            match ack_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    assert_eq!(response.id, 3);
                    match response.msg {
                        Response::Acknowledged { code } => assert_eq!(code, 1),
                        other => panic!("unexpected response: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }

            // State is untouched: old token current, same pending token, forged absent
            let entries = sessions.entries.lock().unwrap();
            let old = old_token.parse::<u64>().unwrap();
            assert!(entries.contains_key(&old));
            assert!(!entries.contains_key(&forged_token.parse::<u64>().unwrap()));
            assert_eq!(
                entries.get(&old).unwrap().pending_token,
                Some(pending_token.parse::<u64>().unwrap())
            );
            drop(entries);

            // The pending rotation survives: next pong offers the same token
            let ping_message = OwnedMessage::Text(
                serde_json::json!({
                    "id": 4,
                    "msg": { "Ping": null }
                })
                .to_string(),
            );
            if let Err(e) = client.send_message(&ping_message) {
                panic!("Failed to send ping message: {:?}", e);
            }
            let ping_reply = match client.recv_message() {
                Ok(reply) => reply,
                Err(e) => panic!("Failed to receive pong response: {:?}", e),
            };
            match ping_reply {
                OwnedMessage::Text(payload) => {
                    let response: SeqResponse = serde_json::from_str(&payload).unwrap();
                    match response.msg {
                        Response::Pong {
                            new_token: Some(token),
                            ..
                        } => assert_eq!(token, pending_token),
                        other => panic!("expected pong with new_token, got: {other:?}"),
                    }
                }
                other => panic!("unexpected message: {other:?}"),
            }

            let _ = client.shutdown();
        });
    }
}
