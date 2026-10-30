//! 从敏感连接串提取非秘密的目标身份（主机、端口、库名、账号）；口令只在 libpq 适配器中短暂使用。

use anyhow::Context;

/// PostgreSQL 连接目标的非秘密部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabaseTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) username: String,
}

impl DatabaseTarget {
    /// 与配置 loader 相同的查询覆盖语义：`host`、`port`、`dbname`、`user` 优先于 URI 成分。
    pub(crate) fn parse(url: &str) -> anyhow::Result<Self> {
        let invalid = || anyhow::anyhow!("数据库连接参数无效");
        let parsed = url::Url::parse(url).map_err(|_| invalid())?;
        let mut host = match parsed.host() {
            Some(url::Host::Ipv6(address)) => address.to_string(),
            Some(_) => decode_component(parsed.host_str().unwrap_or_default())?,
            None => String::new(),
        };
        let mut port = parsed.port().unwrap_or(5432);
        let mut database = decode_component(parsed.path().trim_start_matches('/'))?;
        let mut username = decode_component(parsed.username())?;
        let mut host_address = None;
        for (key, value) in parsed.query_pairs() {
            match key.as_ref() {
                "host" => host = value.into_owned(),
                "hostaddr" => host_address = Some(value.into_owned()),
                "port" => port = value.parse().map_err(|_| invalid())?,
                "dbname" => database = value.into_owned(),
                "user" => username = value.into_owned(),
                _ => {}
            }
        }
        // 只给出 hostaddr 时以它作为目标身份；同时给出时 host 保留给 TLS 校验与 passfile 匹配。
        if host.is_empty() {
            host = host_address.unwrap_or_default();
        }
        if host.is_empty() || port == 0 || database.is_empty() || database.contains('/') {
            return Err(invalid());
        }
        Ok(Self { host, port, database, username })
    }

    /// 人工确认的库名必须与配置目标逐字一致。
    pub(crate) fn require_database(&self, confirmed: &str) -> anyhow::Result<()> {
        if self.database != confirmed {
            anyhow::bail!("--database 与 Worker 配置中的数据库名不一致");
        }
        Ok(())
    }

    /// 维护连接必须指向同一 PostgreSQL 实例（同主机、同端口）。
    pub(crate) fn require_same_endpoint(&self, other: &Self) -> anyhow::Result<()> {
        if !self.host.eq_ignore_ascii_case(&other.host) || self.port != other.port {
            anyhow::bail!("维护凭据与应用数据库必须指向同一 PostgreSQL 主机与端口");
        }
        Ok(())
    }
}

/// 解码 URI 百分号转义；非法转义、非 UTF-8 或控制字符一律拒绝，错误不回显原文。
pub(crate) fn decode_component(value: &str) -> anyhow::Result<String> {
    let raw = value.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            let pair = raw
                .get(index + 1..index + 3)
                .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
                .context("数据库连接参数含无效的百分号转义")?;
            let text = std::str::from_utf8(pair).context("数据库连接参数含无效的百分号转义")?;
            bytes.push(u8::from_str_radix(text, 16).context("数据库连接参数含无效的百分号转义")?);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).context("数据库连接参数不是 UTF-8")?;
    if decoded.chars().any(char::is_control) {
        anyhow::bail!("数据库连接参数含控制字符");
    }
    Ok(decoded)
}
