//! 数据密钥环的只读状态与离线重加密参数；不读取配置、不连接数据库。

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

/// 可显式指定的数据密钥域；`mail_diagnostics` 的归档文件不在数据库重加密范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DataKeyDomainArg {
    Equipment,
    UserLogin,
    Credential,
    MailDiagnostics,
}

#[cfg(feature = "postgres")]
impl DataKeyDomainArg {
    pub(crate) const fn domain(self) -> auction_config::DataKeyDomain {
        match self {
            Self::Equipment => auction_config::DataKeyDomain::Equipment,
            Self::UserLogin => auction_config::DataKeyDomain::UserLogin,
            Self::Credential => auction_config::DataKeyDomain::Credential,
            Self::MailDiagnostics => auction_config::DataKeyDomain::MailDiagnostics,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct KeysReencryptArgs {
    /// 目标域；省略时依次处理 equipment、user_login、credential。
    #[arg(long, value_enum)]
    pub(crate) domain: Option<DataKeyDomainArg>,
    /// 实际写库；省略时只演练并解密验证，不写入任何信封。
    #[arg(long)]
    pub(crate) apply: bool,
    /// 新建的回执 JSON；写入时必填，已存在即拒绝覆盖。
    #[arg(long, value_name = "FILE")]
    pub(crate) receipt: Option<PathBuf>,
    /// 审计操作者标识；写入时必填。
    #[arg(long)]
    pub(crate) actor: Option<String>,
    /// 每页扫描行数；存储层会截断到固定上限。
    #[arg(long, default_value_t = 200)]
    pub(crate) page_size: i64,
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// 报告各域当前/旧密钥、数据库实际引用的 key ID 与保留期；--strict 对缺失或过期引用失败。
    Status {
        /// 任一信封引用未知 key，或保留期已过但仍有引用时失败。
        #[arg(long)]
        strict: bool,
    },
    /// 离线把仍引用旧 key 的信封重加密为当前 key；默认演练，--apply 才写入并追加审计。
    Reencrypt(KeysReencryptArgs),
}
