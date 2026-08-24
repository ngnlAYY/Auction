//! 进程角色闭集与默认监听策略。

use std::str::FromStr;

use super::super::ConfigError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Public,
    Admin,
    Worker,
}

impl Role {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    pub(crate) const fn default_listen(self) -> Option<&'static str> {
        match self {
            Self::Public => Some("127.0.0.1:8090"),
            Self::Admin => Some("127.0.0.1:8091"),
            Self::Worker => None,
        }
    }
}

impl FromStr for Role {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "public" => Ok(Self::Public),
            "admin" => Ok(Self::Admin),
            "worker" => Ok(Self::Worker),
            _ => Err(ConfigError::UnknownRole(value.to_owned())),
        }
    }
}
