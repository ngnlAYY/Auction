//! MoogleMail HTML 文档解析器的职责分组。

use std::{
    collections::{HashMap, HashSet},
    fmt, str,
};

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use scraper::{ElementRef, Html, Selector};
use serde::{
    Deserializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use sha2::{Digest, Sha256};
use url::Url;

use super::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
    protocol::{
        MoogleMailEquipmentKey, MoogleMailToken, valid_decimal_id, valid_hex_key, valid_line,
        valid_opaque_id,
    },
};

/// 无状态的 MoogleMail HTML 解析器。
#[derive(Debug, Clone, Copy, Default)]
pub struct MoogleMailParser;

mod actions;
mod common;
mod detail_helpers;
mod detail_parse;
mod list_helpers;
mod reads;
mod script;
mod types;

pub(crate) use common::*;
pub(crate) use detail_helpers::*;
pub(crate) use detail_parse::*;
pub(crate) use list_helpers::*;
pub(crate) use script::*;
pub use types::*;
