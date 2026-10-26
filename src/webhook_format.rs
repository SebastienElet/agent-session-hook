// Bearer + JSON {worktree, event, time}: verify this format at the first real webhook test.
use crate::{Error, failure};
use serde::Serialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
pub(crate) struct Payload {
    worktree: PathBuf,
    event: &'static str,
    time: u64,
}

pub(crate) struct Encoded {
    pub(crate) authorization: String,
    pub(crate) body: Vec<u8>,
}

impl Payload {
    pub(crate) fn new(worktree: PathBuf, event: &'static str) -> Result<Self, Error> {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| failure("clock is unavailable"))?
            .as_secs();
        Ok(Self {
            worktree,
            event,
            time,
        })
    }

    pub(crate) fn encode(&self, token: &str) -> Result<Encoded, Error> {
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(failure("invalid webhook token"));
        }
        let body = serde_json::to_vec(self).map_err(|_| failure("payload encoding failed"))?;
        Ok(Encoded {
            authorization: format!("Bearer {token}"),
            body,
        })
    }
}
