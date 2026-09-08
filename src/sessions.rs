pub mod protocol;
pub mod sshclient;
use protocol::{Request, Response, SeqRequest, SeqResponse};
use rand::prelude::*;
use ssh2::Session as Shell;
use sshclient::{RunOutput, RunResult, ShellProvider};
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};
use log::{debug, info};

pub struct Session {
    pub shell: Shell,
    provider: Arc<dyn ShellProvider>,
}

pub struct Sessions {
    pub entries: Mutex<HashMap<u64, Shell>>,
    pub target: String,
    pub debug: bool,
    pub provider: Arc<dyn ShellProvider>,
}

fn handle_anonymous_command<'a>(
    request: Request,
    sessions: &'a Sessions,
) -> (Response, Option<Session>) {
    match request {
        Request::Previous { token } => sessions.get_session(token),
        Request::Ping(_) => (Response::Pong(Some(0)), None),
        Request::Login { user, passwd } => sessions.do_login(user, passwd),
        Request::Run { cmd: _cmd } => (Response::NotAllowed, None),
    }
}

fn handle_session_command(request: Request, session: &Session) -> Response {
    match request {
        Request::Ping(_) => Response::Pong(Some(0)),
        Request::Run { cmd } => session.run_command(&cmd),
        _ => Response::NotImplemented,
    }
}

pub fn attend_message<'a>(command: String, sessions: &'a Sessions) -> (String, Option<Session>) {
    let request: SeqRequest = serde_json::from_str(&command).unwrap();
    debug!(
        "Requested: {:?}",
        serde_json::to_string_pretty(&request).unwrap()
    );
    let (outcome, new_state) = handle_anonymous_command(request.msg, sessions);
    let response = SeqResponse {
        id: request.id,
        msg: outcome,
    };
    debug!("Replied: {:?}", response);
    (serde_json::to_string(&response).unwrap(), new_state)
}

pub fn attend_session_message(command: String, session: &Session) -> String {
    let request: SeqRequest = serde_json::from_str(&command).unwrap();
    debug!(
        "Requested: {:?}",
        serde_json::to_string_pretty(&request).unwrap()
    );
    let outcome = handle_session_command(request.msg, session);
    let response = SeqResponse {
        id: request.id,
        msg: outcome,
    };
    debug!("Replied: {:?}", response);
    serde_json::to_string(&response).unwrap()
}

impl Session {
    pub fn new(shell: Shell, provider: Arc<dyn ShellProvider>) -> Self {
        Session { shell, provider }
    }
    pub fn run_command(&self, command: &str) -> Response {
        let _ = &self.shell;
        match self.provider.run_command(&self.shell, command) {
            RunResult::Ok(RunOutput(response, code)) => {
                info!("DONE {command}: {response}");
                Response::Outcome { response, code }
            }
            RunResult::Err(RunOutput(reason, code)) => {
                info!("FAILED: {:?}", reason);
                Response::Outcome {
                    response: reason,
                    code,
                }
            }
        }
    }
}

fn parse_token(source: Option<String>) -> Option<u64> {
    source.and_then(|input| input.parse::<u64>().ok())
}

impl Sessions {
    pub fn new(
        target: &str,
        provider: impl ShellProvider + Send + 'static,
        debug: bool,
    ) -> Sessions {
        let box_to_provider: Arc<dyn ShellProvider + Send> = Arc::new(provider);
        Sessions {
            entries: Mutex::new(HashMap::new()),
            target: target.to_string(),
            provider: box_to_provider,
            debug,
        }
    }
    fn store_session(&self, session: Shell) -> u64 {
        let entries = &mut self.entries.lock().unwrap();
        let mut rng = rand::thread_rng();
        let token = rng.gen::<u64>();
        entries.insert(token, session);
        token
    }
    pub fn do_login<'a>(&self, user: String, password: String) -> (Response, Option<Session>) {
        match self.provider.connect(&self.target, &user, &password) {
            Err(RunOutput(response, code)) => (
                Response::Authentication {
                    code,
                    token: None,
                    reason: response.to_string(),
                    new_session: true,
                },
                None,
            ),
            Ok(opened_shell) => {
                let token = Some(self.store_session(opened_shell.clone()).to_string());
                (
                    Response::Authentication {
                        code: 0,
                        token,
                        reason: "Done.".to_string(),
                        new_session: false,
                    },
                    Some(Session::new(opened_shell.clone(), self.provider.clone())),
                )
            }
        }
    }
    pub fn get_session<'a>(&self, token_str: Option<String>) -> (Response, Option<Session>) {
        if let Some(token) = parse_token(token_str) {
            let entries = &mut self.entries.lock().unwrap();
            match entries.get(&token) {
                Some(shell) => (
                    Response::Previous {
                        code: 0,
                        reason: "none".to_string(),
                        new_session: false,
                        platform: "generic".to_string(),
                        host: "unknown".to_string(),
                    },
                    Some(Session::new(shell.to_owned(), self.provider.clone())),
                ),
                None => (
                    Response::Previous {
                        code: 1,
                        reason: "none".to_string(),
                        new_session: true,
                        platform: "generic".to_string(),
                        host: "unknown".to_string(),
                    },
                    None,
                ),
            }
        } else {
            (
                Response::Previous {
                    code: 1,
                    reason: "Invalid token".to_string(),
                    new_session: true,
                    platform: "generic".to_string(),
                    host: "unknown".to_string(),
                },
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::sshclient::ShellProvider;
    use mockall::mock;
    use mockall::predicate::*;

    mock! {
        ShellProvider {}
        impl ShellProvider for ShellProvider {
            fn connect(&self, host: &str, user: &str, password: &str) -> Result<Shell, RunOutput>;
            fn run_command(&self, shell: &Shell, command: &str) -> RunResult;
        }
    }

    #[test]
    fn test_create_session() {
        let mut mock_ssh_connect = MockShellProvider::new();

        // Set up expectations for the mock object
        mock_ssh_connect
            .expect_connect()
            .with(eq("localhost:22"), eq("user"), eq("password"))
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new("localhost:22", mock_ssh_connect, true);

        // Set expectations for the mock `ssh_connect` function

        let (response, session_result) =
            sessions.do_login("user".to_string(), "password".to_string());
        match session_result {
            Some(_session) => assert!(true),
            None => assert!(true),
        }
        let entries = sessions.entries.lock().unwrap();
        debug!("{:?}", entries.keys());

        if let Response::Authentication {
            token: maybe_token,
            code: 0,
            ..
        } = response
        {
            debug!("maybe token: {:?}", maybe_token);
            let Some(token) = maybe_token else { assert!(false); return; };
            debug!("{:?}", token);
        } else {
            debug!("incorrect response: {:?}", response);
            assert!(false)
        };
        let entries_count = entries.keys().len();
        assert!(entries_count == 1)
    }

    #[test]
    fn test_create_session_failed() {
        let mut mock_ssh_connect = MockShellProvider::new();

        // Set up expectations for the mock object
        mock_ssh_connect
            .expect_connect()
            .with(eq("localhost:22"), eq("user"), eq("wrong"))
            .returning(|_, _, _| {
                let code = -18;
                let response = String::from("Authentication failed (keyboard-interactive)");
                Err(RunOutput(response, code))
            });

        let sessions = Sessions::new("localhost:22", mock_ssh_connect, true);

        // Set expectations for the mock `ssh_connect` function

        let (response, session_result) = sessions.do_login("user".to_string(), "wrong".to_string());
        match session_result {
            Some(_session) => assert!(true),
            None => assert!(true),
        }
        let entries = sessions.entries.lock().unwrap();
        debug!("{:?}", entries.keys());

        if let Response::Authentication {
            token: None,
            code: -18,
            reason,
            ..
        } = response
        {
            debug!("{:?}", reason);
        } else {
            debug!("incorrect response: {:?}", response);
            assert!(false)
        };
        let entries_count = entries.keys().len();
        assert!(entries_count == 0)
    }
}
