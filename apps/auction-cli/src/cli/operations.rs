//! 业务运维命令的参数定义；所有写入都要求稳定命令 ID、操作者与逐字确认。

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum CredentialCommand {
    /// 列出凭据安全投影；不输出密文、明文或密钥指纹。
    List {
        #[arg(long, value_enum)]
        kind: Option<CredentialKindArg>,
        #[arg(long, value_enum)]
        world: Option<CredentialWorldArg>,
        #[arg(long, value_enum)]
        status: Option<CredentialStatusArg>,
        /// 每页条数，1-100。
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(i64).range(1..=100))]
        limit: i64,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(i64).range(0..=10_000))]
        offset: i64,
    },
    /// 从 stdin 读取 Cookie 并用 credential 数据密钥加密导入；Worker 重启后生效。
    Import {
        #[arg(long, value_enum)]
        kind: CredentialKindArg,
        /// 凭据适用范围；shared 同时服务 main 与 isekai。
        #[arg(long, value_enum)]
        world: CredentialWorldArg,
        /// Admin 与 Worker 中显示的稳定标签。
        #[arg(long)]
        label: String,
        /// 可选的预期角色名；只用于显示与核对，不代表已远端验证。
        #[arg(long)]
        player_name: Option<String>,
        /// 写入审计的操作者标识。
        #[arg(long, default_value = "auction-cli")]
        actor: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CredentialKindArg {
    /// MoogleMail 游戏邮件 Cookie。
    Mail,
    /// E-Hentai 论坛 Cookie（ipb_member_id 与 ipb_pass_hash）。
    Forum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CredentialWorldArg {
    Main,
    Isekai,
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CredentialStatusArg {
    Active,
    Disabled,
    Invalid,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RulesCommand {
    /// 从包含 en/zh-CN/zh-TW/ja/ko 五个 Markdown 的目录首次发布规则；不覆盖已有的不同正文。
    Import {
        /// 规则目录。
        #[arg(long, value_name = "DIR")]
        directory: PathBuf,
        /// 本批次非零 UUID；各语言命令 ID 由它稳定派生，重试必须复用。
        #[arg(long)]
        command_id: Uuid,
        /// 写入发布审计的操作者标识。
        #[arg(long)]
        actor: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ForumCommand {
    /// 只读列出写入未知、对账中或等待人工授权的论坛主题任务。
    Recovery {
        /// 最多返回条数，1-100。
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    /// 精确标题对账未命中后，授权 Worker 再尝试一次创建主题。
    AuthorizeRetry {
        /// 处于 write_authorization_required 的拍卖 UUID。
        #[arg(long)]
        auction_id: Uuid,
        /// 稳定命令 UUID；重复提交只重放同一请求。
        #[arg(long)]
        command_id: Uuid,
        /// 写入审计的操作者标识。
        #[arg(long)]
        actor: String,
        /// 逐字确认：`AUTHORIZE FORUM WRITE <拍卖UUID>`。
        #[arg(long, value_name = "TEXT")]
        confirm: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PmCommand {
    /// 列出装备 PM 投递的安全状态投影；不输出正文、收件人或租约 token。
    List {
        /// 可选状态过滤，例如 manual_reconcile。
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(i64).range(1..=100))]
        limit: i64,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(i64).range(0..=10_000))]
        offset: i64,
    },
    /// 请求 Worker 以原发送者只读核对一条 manual_reconcile 私信；绝不重发。
    Reconcile {
        #[arg(long)]
        notification_id: Uuid,
        /// 稳定命令 UUID；未知结果按原参数重试。
        #[arg(long)]
        command_id: Uuid,
        /// list 输出中的 fencing_version。
        #[arg(long)]
        expected_fencing_version: i64,
        /// 写入审计的操作者标识。
        #[arg(long)]
        actor: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum MailCommand {
    /// 按证据 UUID 解密导出 MoogleMail 诊断归档；输出文件已存在时拒绝覆盖。
    ExportEvidence {
        /// Worker 日志中的 evidence_id。
        evidence_id: Uuid,
        /// 新建的 JSON 输出文件。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
        /// 历史父密钥来源；省略时优先 credential，缺失才用 equipment。
        #[arg(long, value_enum)]
        key_source: Option<KeySourceArg>,
    },
    /// 只读核查撤拍退回义务在 Sent 中的精确邮件；不重发、不改履约状态。
    InspectSent {
        /// withdrawn_return 义务 UUID。
        obligation_id: Uuid,
        /// 新建的安全 JSON 报告；不含正文、Cookie 或装备 key。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum KeySourceArg {
    Credential,
    Equipment,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FundsCommand {
    /// 数据库只读责任预览；输出版本、摘要与可用的确认短语，不访问远端。
    Preview {
        /// Bot 账号的游戏 UID（目标断言，不替代远端认证）。
        #[arg(long)]
        uid: String,
    },
    /// 为尚无资金账户的 UID 建立 record_only 账户；需要停止 Worker 后执行。
    OpenRecordOnly {
        #[command(flatten)]
        write: FundsWriteArgs,
    },
    /// 以真实 Main 现金与联合身份完成 strict_cash 对账；需要停止 Worker 后执行。
    Reconcile {
        #[command(flatten)]
        write: FundsWriteArgs,
        /// 本次授权的 Bot 自有额度（Credits）。
        #[arg(long)]
        proprietary_credits: i64,
        /// 本次授权的补偿池额度（Credits）。
        #[arg(long)]
        compensation_credits: i64,
        /// 声明专用账号无并发人工资金写入、无未登记外部负债，且全部程序已同版升级。
        #[arg(long)]
        dedicated_account: bool,
    },
}

/// 资金写入共用的稳定身份与预览绑定。
#[derive(Debug, Args)]
pub(crate) struct FundsWriteArgs {
    /// Bot 账号的游戏 UID。
    #[arg(long)]
    pub(crate) uid: String,
    /// 稳定非零命令 UUID；未知结果必须用原 ID 与全部参数重试。
    #[arg(long)]
    pub(crate) command_id: Uuid,
    /// 写入审计的操作者标识。
    #[arg(long)]
    pub(crate) actor: String,
    /// preview 输出的 account_version。
    #[arg(long)]
    pub(crate) expected_version: i64,
    /// preview 输出的 64 位小写十六进制 preview_digest。
    #[arg(long)]
    pub(crate) preview_digest: String,
    /// 逐字使用 preview 输出的确认短语。
    #[arg(long, value_name = "TEXT")]
    pub(crate) confirm: String,
}
