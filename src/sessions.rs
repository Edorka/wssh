pub mod protocol;
pub mod sshclient;
use protocol::{Request, Response, SeqRequest, SeqResponse};
use rand::prelude::*;
use ssh2::Session as Shell;
use sshclient::{RunOutput, RunResult, ShellProvider};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{collections::HashMap, sync::Mutex};
use log::{debug, error, info};

pub struct Session {
    pub shell: Shell,
    pub token: u64,
    provider: Arc<dyn ShellProvider>,
}

pub struct SessionEntry {
    pub shell: Shell,
    pub pending_token: Option<u64>,
    pub last_rotation: Instant,
}

pub struct Sessions {
    pub entries: Mutex<HashMap<u64, SessionEntry>>,
    pub target: String,
    pub debug: bool,
    pub provider: Arc<dyn ShellProvider>,
    pub rotation_period: Option<Duration>,
}

fn handle_anonymous_command<'a>(
    request: Request,
    sessions: &'a Sessions,
) -> (Response, Option<Session>) {
    match request {
        Request::Previous { token } => sessions.get_session(token),
        Request::Ping(_) => (
            Response::Pong {
                value: Some(0),
                new_token: None,
            },
            None,
        ),
        Request::Login { user, passwd } => sessions.do_login(user, passwd),
        Request::Acknowledge { .. } => (Response::NotAllowed, None),
        Request::Run { cmd: _cmd } => (Response::NotAllowed, None),
    }
}

fn handle_session_command(
    request: Request,
    session: &Session,
    sessions: &Sessions,
) -> (Response, Option<u64>) {
    match request {
        Request::Ping(_) => (
            Response::Pong {
                value: Some(0),
                new_token: sessions.maybe_rotate(session.token).map(|t| t.to_string()),
            },
            None,
        ),
        Request::Run { cmd } => (session.run_command(&cmd), None),
        Request::Acknowledge { token } => sessions.acknowledge_rotation(session.token, token),
        _ => (Response::NotImplemented, None),
    }
}

pub fn attend_message<'a>(command: String, sessions: &'a Sessions) -> (String, Option<Session>) {
    let request: Result<SeqRequest, _> = serde_json::from_str(&command);
    match request {
        Ok(req) => {
            debug!(
                "Requested: {:?}",
                serde_json::to_string_pretty(&req).unwrap()
            );
            let (outcome, new_state) = handle_anonymous_command(req.msg, sessions);
            let response = SeqResponse {
                id: req.id,
                msg: outcome,
            };
            debug!("Replied: {:?}", response);
            (serde_json::to_string(&response).unwrap(), new_state)
        }
        Err(e) => {
            error!("Failed to parse JSON request: {:?}", e);
            let response = SeqResponse {
                id: 0, // Default ID since we couldn't parse the request
                msg: Response::NotAllowed, // Or another appropriate error response
            };
            debug!("Replied with error: {:?}", response);
            (serde_json::to_string(&response).unwrap(), None)
        }
    }
}

pub fn attend_session_message(
    command: String,
    session: &Session,
    sessions: &Sessions,
) -> (String, Option<u64>) {
    let request: Result<SeqRequest, _> = serde_json::from_str(&command);
    match request {
        Ok(req) => {
            debug!(
                "Requested: {:?}",
                serde_json::to_string_pretty(&req).unwrap()
            );
            let (outcome, rotated) = handle_session_command(req.msg, session, sessions);
            let response = SeqResponse {
                id: req.id,
                msg: outcome,
            };
            debug!("Replied: {:?}", response);
            (serde_json::to_string(&response).unwrap(), rotated)
        }
        Err(e) => {
            error!("Failed to parse JSON request: {:?}", e);
            let response = SeqResponse {
                id: 0, // Default ID since we couldn't parse the request
                msg: Response::NotAllowed, // Or another appropriate error response
            };
            debug!("Replied with error: {:?}", response);
            (serde_json::to_string(&response).unwrap(), None)
        }
    }
}

impl Session {
    pub fn new(shell: Shell, provider: Arc<dyn ShellProvider>, token: u64) -> Self {
        Session {
            shell,
            token,
            provider,
        }
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
        rotation_period: Option<Duration>,
    ) -> Sessions {
        let box_to_provider: Arc<dyn ShellProvider + Send> = Arc::new(provider);
        Sessions {
            entries: Mutex::new(HashMap::new()),
            target: target.to_string(),
            provider: box_to_provider,
            debug,
            rotation_period,
        }
    }
    fn store_session(&self, session: Shell) -> u64 {
        let entries = &mut self.entries.lock().unwrap();
        let mut rng = rand::thread_rng();
        let token = rng.gen::<u64>();
        entries.insert(
            token,
            SessionEntry {
                shell: session,
                pending_token: None,
                last_rotation: Instant::now(),
            },
        );
        token
    }
    fn maybe_rotate(&self, token: u64) -> Option<u64> {
        let period = self.rotation_period?;
        let entries = &mut self.entries.lock().unwrap();
        let entry = entries.get_mut(&token)?;
        if let Some(pending) = entry.pending_token {
            return Some(pending);
        }
        if entry.last_rotation.elapsed() < period {
            return None;
        }
        let mut rng = rand::thread_rng();
        let new_token = rng.gen::<u64>();
        entry.pending_token = Some(new_token);
        Some(new_token)
    }
    fn acknowledge_rotation(
        &self,
        current_token: u64,
        token: Option<String>,
    ) -> (Response, Option<u64>) {
        let Some(new_token) = parse_token(token) else {
            return (Response::Acknowledged { code: 1 }, None);
        };
        let entries = &mut self.entries.lock().unwrap();
        let pending = match entries.get(&current_token) {
            Some(entry) => entry.pending_token,
            None => return (Response::Acknowledged { code: 1 }, None),
        };
        if pending != Some(new_token) {
            return (Response::Acknowledged { code: 1 }, None);
        }
        let mut entry = entries.remove(&current_token).unwrap();
        entry.pending_token = None;
        entry.last_rotation = Instant::now();
        entries.insert(new_token, entry);
        info!("Session token rotated: {} -> {}", current_token, new_token);
        (Response::Acknowledged { code: 0 }, Some(new_token))
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
                let new_token = self.store_session(opened_shell.clone());
                (
                    Response::Authentication {
                        code: 0,
                        token: Some(new_token.to_string()),
                        reason: "Done.".to_string(),
                        new_session: false,
                    },
                    Some(Session::new(
                        opened_shell.clone(),
                        self.provider.clone(),
                        new_token,
                    )),
                )
            }
        }
    }
    pub fn get_session<'a>(&self, token_str: Option<String>) -> (Response, Option<Session>) {
        if let Some(token) = parse_token(token_str) {
            let entries = &mut self.entries.lock().unwrap();
            match entries.get(&token) {
                Some(entry) => (
                    Response::Previous {
                        code: 0,
                        reason: "none".to_string(),
                        new_session: false,
                        platform: "generic".to_string(),
                        host: "unknown".to_string(),
                    },
                    Some(Session::new(
                        entry.shell.to_owned(),
                        self.provider.clone(),
                        token,
                    )),
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

        let sessions = Sessions::new("localhost:22", mock_ssh_connect, true, None);

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

        let sessions = Sessions::new("localhost:22", mock_ssh_connect, true, None);

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

    #[test]
    fn test_no_rotation_before_period_elapsed() {
        let mut mock_ssh_connect = MockShellProvider::new();
        mock_ssh_connect
            .expect_connect()
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new(
            "localhost:22",
            mock_ssh_connect,
            true,
            Some(Duration::from_secs(3600)),
        );
        let (_response, session) = sessions.do_login("user".to_string(), "password".to_string());
        let session = session.unwrap();
        assert_eq!(sessions.maybe_rotate(session.token), None);
    }

    #[test]
    fn test_rotation_disabled() {
        let mut mock_ssh_connect = MockShellProvider::new();
        mock_ssh_connect
            .expect_connect()
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new("localhost:22", mock_ssh_connect, true, None);
        let (_response, session) = sessions.do_login("user".to_string(), "password".to_string());
        let session = session.unwrap();
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(sessions.maybe_rotate(session.token), None);
    }

    #[test]
    fn test_token_rotation_pending_and_acknowledge() {
        let mut mock_ssh_connect = MockShellProvider::new();
        mock_ssh_connect
            .expect_connect()
            .returning(|_, _, _| Ok(Shell::new().unwrap()));

        let sessions = Sessions::new(
            "localhost:22",
            mock_ssh_connect,
            true,
            Some(Duration::from_millis(1)),
        );
        let (_response, session) = sessions.do_login("user".to_string(), "password".to_string());
        let session = session.unwrap();
        std::thread::sleep(Duration::from_millis(10));

        // period elapsed: a pending token is created
        let pending = sessions.maybe_rotate(session.token).unwrap();
        // while pending, the same token is offered again
        assert_eq!(sessions.maybe_rotate(session.token), Some(pending));
        // old token still resolves
        let (response, previous) = sessions.get_session(Some(session.token.to_string()));
        assert!(matches!(response, Response::Previous { code: 0, .. }));
        assert!(previous.is_some());

        // a wrong acknowledge is rejected and changes nothing
        let wrong_token = pending.wrapping_add(1).to_string();
        let (response, rotated) = sessions.acknowledge_rotation(session.token, Some(wrong_token));
        assert!(matches!(response, Response::Acknowledged { code: 1 }));
        assert_eq!(rotated, None);
        let (response, previous) = sessions.get_session(Some(session.token.to_string()));
        assert!(matches!(response, Response::Previous { code: 0, .. }));
        assert!(previous.is_some());

        // the correct acknowledge makes the rotation effective
        let (response, rotated) =
            sessions.acknowledge_rotation(session.token, Some(pending.to_string()));
        assert!(matches!(response, Response::Acknowledged { code: 0 }));
        assert_eq!(rotated, Some(pending));

        // old token is rejected, new token resolves
        let (response, previous) = sessions.get_session(Some(session.token.to_string()));
        assert!(matches!(response, Response::Previous { code: 1, .. }));
        assert!(previous.is_none());
        let (response, previous) = sessions.get_session(Some(pending.to_string()));
        assert!(matches!(response, Response::Previous { code: 0, .. }));
        assert!(previous.is_some());
    }
}
