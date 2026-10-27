use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Response {
    #[serde(rename_all = "camelCase")]
    Previous {
        code: i32,
        reason: String,
        new_session: bool,
        platform: String,
        host: String,
    },
    Pong {
        value: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_token: Option<String>,
    },
    Authentication {
        code: i32,
        token: Option<String>,
        new_session: bool,
        reason: String,
    },
    Acknowledged {
        code: i32,
    },
    Outcome {
        code: i32,
        response: String,
    },
    NotImplemented,
    NotAllowed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Previous { token: Option<String> },
    Ping(Option<i32>),
    Acknowledge { token: Option<String> },
    Login { user: String, passwd: String },
    Run { cmd: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SeqRequest {
    pub id: u64,
    pub msg: Request,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SeqResponse {
    pub id: u64,
    pub msg: Response,
}
