extern crate websocket;

use crate::sessions::{attend_message, attend_session_message, Session, Sessions};
use std::net::TcpStream;
use std::thread;
use websocket::receiver::Reader;
use websocket::sender::Writer;
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
                println!("Error processing: {:?}", e);
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
        //let mut current: Option<Session> = None;
        let (mut receiver, mut sender) = connection.split().unwrap();
        let achieved = self.attend_messages(&mut receiver, &mut sender);
        match achieved {
            Some(session) => self.attend_session_messages(&mut receiver, &mut sender, session),
            _ => (),
        };
    }

    pub fn run(&self) -> () {
        let address = &self.address;
        let instance = Server::bind(address).unwrap();
        println!("Websocket Server listening on: {address}");
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
}
