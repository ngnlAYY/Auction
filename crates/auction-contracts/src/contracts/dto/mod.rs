//! HTTP/流契约 DTO 的分组入口；对外仍由 `contracts` 统一 re-export。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod admin;
mod public;
mod shared;

pub use admin::*;
pub use public::*;
pub use shared::*;
