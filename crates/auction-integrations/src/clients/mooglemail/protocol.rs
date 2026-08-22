use std::{fmt, net::IpAddr};

use url::Url;

use super::{MoogleMailError, MoogleMailErrorCategory};

const PRODUCTION_HOST: &str = "hentaiverse.org";
const MAX_COOKIE_BYTES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 1024;
const MAX_OPAQUE_ID_BYTES: usize = 256;

/// MoogleMail 的游戏世界闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoogleMailWorld {
    /// 主世界，对应根路径。
    Main,
    /// Isekai 世界，对应 `/isekai/` 路径。
    Isekai,
}

impl MoogleMailWorld {
    /// 返回外部协议中的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }

    /// 解析外部协议中的世界名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "isekai" => Some(Self::Isekai),
            _ => None,
        }
    }

    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Main => "/",
            Self::Isekai => "/isekai/",
        }
    }
}

impl fmt::Display for MoogleMailWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// MoogleMail 读取和写入协议的操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoogleMailOperation {
    /// 收件箱分页。
    Inbox,
    /// 单封收件箱详情。
    Detail,
    /// 已发送邮件分页。
    Sent,
    /// 单封已发送邮件详情。
    SentDetail,
    /// Item Shop 余额页面。
    Balance,
    /// 装备详情页面。
    Equipment,
    /// 新邮件页面。
    Compose,
    /// compose 页面附加 Credits。
    AttachAdd,
    /// compose 页面设置 CoD。
    AttachCod,
    /// compose 页面最终发送。
    Send,
    /// 详情页面退回整封邮件。
    ReturnMessage,
    /// 详情页面领取全部附件。
    Take,
}

impl MoogleMailOperation {
    /// 返回稳定的操作名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Detail => "detail",
            Self::Sent => "sent",
            Self::SentDetail => "sent_detail",
            Self::Balance => "balance",
            Self::Equipment => "equipment",
            Self::Compose => "compose",
            Self::AttachAdd => "attach_add",
            Self::AttachCod => "attach_cod",
            Self::Send => "send",
            Self::ReturnMessage => "return_message",
            Self::Take => "attach_remove",
        }
    }
}

impl fmt::Display for MoogleMailOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 仅在受控边界内临时使用的 MoogleMail Cookie。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailCookie(Box<str>);

impl MoogleMailCookie {
    /// 校验 Cookie 行并建立脱敏值对象。
    pub fn new(value: impl Into<String>) -> Result<Self, MoogleMailError> {
        let value = value.into();
        if !valid_secret(&value, MAX_COOKIE_BYTES) {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::CredentialUnavailable,
                false,
            ));
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MoogleMailCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 详情页面中的短生命周期 `mmtoken`。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailToken(Box<str>);

impl MoogleMailToken {
    pub(crate) fn new(value: &str) -> Option<Self> {
        valid_secret(value, MAX_TOKEN_BYTES).then(|| Self(value.to_owned().into_boxed_str()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MoogleMailToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// MoogleMail 装备附件的 10 位十六进制 key。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailEquipmentKey([u8; 10]);

impl MoogleMailEquipmentKey {
    /// 校验并规范化装备 key 为小写 ASCII。
    pub fn new(value: &str) -> Option<Self> {
        if value.len() != 10 || !value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut normalized = [0_u8; 10];
        for (index, byte) in value.bytes().enumerate() {
            normalized[index] = byte.to_ascii_lowercase();
        }
        Some(Self(normalized))
    }

    /// 只在构造远端装备 URL 的同步边界内暴露 key。
    pub(crate) fn expose(&self) -> &str {
        // `new` 已经保证每个字节都是 ASCII hex；这里使用一次静态转换避免复制 secret。
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// 尽力清除当前内存中的 key。
    pub fn clear(&mut self) {
        self.0.fill(0);
    }
}

impl fmt::Debug for MoogleMailEquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl fmt::Display for MoogleMailEquipmentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 由 URL builder 构造的不透明 MoogleMail 目标。
#[derive(Clone)]
pub struct MoogleMailTarget {
    pub(crate) operation: MoogleMailOperation,
    pub(crate) world: MoogleMailWorld,
    pub(crate) url: Url,
    pub(crate) origin: String,
}

impl MoogleMailTarget {
    /// 返回目标操作，不暴露原始 URL、mid 或装备 key。
    pub const fn operation(&self) -> MoogleMailOperation {
        self.operation
    }

    /// 返回目标世界。
    pub const fn world(&self) -> MoogleMailWorld {
        self.world
    }
}

impl fmt::Debug for MoogleMailTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailTarget")
            .field("operation", &self.operation)
            .field("world", &self.world)
            .finish()
    }
}

impl fmt::Display for MoogleMailTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MoogleMail target: operation={} world={}", self.operation, self.world)
    }
}

/// 只构造批准过的 MoogleMail URL，不接受调用方传入 raw path/query。
#[derive(Clone)]
pub(crate) struct MoogleMailUrlBuilder {
    pub(crate) endpoint: Url,
    pub(crate) origin: String,
}

impl MoogleMailUrlBuilder {
    pub(crate) fn new(endpoint: Url) -> Result<Self, MoogleMailError> {
        if !valid_endpoint(&endpoint) {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Inbox,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        let origin = endpoint_origin(&endpoint);
        Ok(Self { endpoint, origin })
    }

    pub(crate) fn inbox_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut target = self.moogle_mail(world, MoogleMailOperation::Inbox, "inbox", None)?;
        if page > 0 {
            target.url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(target)
    }

    pub(crate) fn detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_opaque_id(external_id) {
            return Err(self.configuration(MoogleMailOperation::Detail, world));
        }
        self.moogle_mail(world, MoogleMailOperation::Detail, "inbox", Some(external_id))
    }

    pub(crate) fn sent_page(
        &self,
        world: MoogleMailWorld,
        page: usize,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut target = self.moogle_mail(world, MoogleMailOperation::Sent, "sent", None)?;
        if page > 0 {
            target.url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(target)
    }

    pub(crate) fn sent_detail(
        &self,
        world: MoogleMailWorld,
        external_id: &str,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_opaque_id(external_id) {
            return Err(self.configuration(MoogleMailOperation::SentDetail, world));
        }
        self.moogle_mail(world, MoogleMailOperation::SentDetail, "sent", Some(external_id))
    }

    pub(crate) fn compose(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        self.moogle_mail(world, MoogleMailOperation::Compose, "new", None)
    }

    pub(crate) fn balance(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        url.query_pairs_mut().append_pair("s", "Bazaar").append_pair("ss", "is");
        Ok(MoogleMailTarget {
            operation: MoogleMailOperation::Balance,
            world,
            url,
            origin: self.origin.clone(),
        })
    }

    pub(crate) fn equipment(
        &self,
        world: MoogleMailWorld,
        equipment_id: &str,
        equipment_key: &MoogleMailEquipmentKey,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        if !valid_decimal_id(equipment_id) {
            return Err(self.configuration(MoogleMailOperation::Equipment, world));
        }
        let mut url = self.endpoint.clone();
        let path = format!(
            "{}/equip/{equipment_id}/{}",
            world.path().trim_end_matches('/'),
            equipment_key.expose()
        );
        url.set_path(&path);
        Ok(MoogleMailTarget {
            operation: MoogleMailOperation::Equipment,
            world,
            url,
            origin: self.origin.clone(),
        })
    }

    fn moogle_mail(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
        filter: &str,
        external_id: Option<&str>,
    ) -> Result<MoogleMailTarget, MoogleMailError> {
        let mut url = self.endpoint.clone();
        url.set_path(world.path());
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("s", "Bazaar").append_pair("ss", "mm").append_pair("filter", filter);
            if let Some(external_id) = external_id {
                query.append_pair("mid", external_id);
            }
        }
        Ok(MoogleMailTarget { operation, world, url, origin: self.origin.clone() })
    }

    fn configuration(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }
}

pub(crate) fn valid_opaque_id(value: &str) -> bool {
    valid_line(value, MAX_OPAQUE_ID_BYTES, false)
}

pub(crate) fn valid_line(value: &str, maximum: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(crate) fn valid_decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn valid_hex_key(value: &str) -> bool {
    value.len() == 10 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_secret(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.bytes().any(|byte| byte == b'\r' || byte == b'\n')
        && !value.chars().any(char::is_control)
}

fn valid_endpoint(endpoint: &Url) -> bool {
    let path_is_root = endpoint.path().is_empty() || endpoint.path() == "/";
    let is_production = endpoint.scheme() == "https"
        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case(PRODUCTION_HOST))
        && endpoint.port().is_none()
        && endpoint.username() == ""
        && endpoint.password().is_none()
        && path_is_root
        && endpoint.query().is_none()
        && endpoint.fragment().is_none();
    if is_production {
        return true;
    }
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || !path_is_root
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return false;
    }
    let Some(hostname) = endpoint.host_str() else {
        return false;
    };
    if hostname.eq_ignore_ascii_case("localhost") {
        return true;
    }
    hostname.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
}

fn endpoint_origin(endpoint: &Url) -> String {
    let mut origin = format!("{}://{}", endpoint.scheme(), endpoint.host_str().unwrap_or_default());
    if let Some(port) = endpoint.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    origin
}
