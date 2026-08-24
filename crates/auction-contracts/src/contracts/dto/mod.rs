//! HTTP/流契约 DTO 的分组入口；对外仍由 `contracts` 统一 re-export。
//!
//! 子模块按公共协议、Public surface 和 Admin surface 拥有 DTO，避免一个文件同时承载所有
//! 浏览器、探针、管理和实时契约。

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
