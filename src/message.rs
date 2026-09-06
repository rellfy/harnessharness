use crate::error::Error;
use serde::Serialize;
use std::str::FromStr;

const USER_ROLE: &str = "user";
const ASSISTANT_ROLE: &str = "assistant";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => USER_ROLE,
            Role::Assistant => ASSISTANT_ROLE,
        }
    }
}

impl FromStr for Role {
    type Err = Error;

    fn from_str(role: &str) -> Result<Self, Error> {
        match role {
            USER_ROLE => Ok(Role::User),
            ASSISTANT_ROLE => Ok(Role::Assistant),
            unknown => Err(Error::UnknownRole(unknown.to_string())),
        }
    }
}
