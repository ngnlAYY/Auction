//! 页面静态挂载命令的参数；不读取配置、不连接任何外部资源。

use std::path::PathBuf;

use clap::{Args, Subcommand};
use url::Url;

use crate::web::materialize::{Surface, SwitchTarget};

/// `auction-cli web`：页面静态挂载的物化、安装、切换与状态。命令不连接数据库、不停服务。
#[derive(Debug, Subcommand)]
pub(crate) enum WebCommand {
    /// 核对新版正式包与来源验证回执：解压目录清单、外层摘要、tag、页面凭据与回执的绑定。
    Verify(VerifyArgs),
    /// 从旧正式二进制的回环源站按旧包清单逐文件物化旧 Dioxus 壳；全部核对通过才安装，不改指针。
    MaterializeLegacy(MaterializeLegacyArgs),
    /// 把新版正式包的页面导出树装成只读 build；首次安装时建立入口指针。
    Install(InstallArgs),
    /// 原子切换某个 surface 的入口指针；切换后须重启 Public/Admin 才生效。
    Switch(SwitchArgs),
    /// 只读：各 surface 的入口指针与各 build 的逐文件核对结果。
    Status(StatusArgs),
    /// 核对冷回切演练回执：绑定 `<web_root>/RECEIPT.v1` 登记的最终实物，逐响应核对策略与构建号。
    DrillVerify(DrillVerifyArgs),
}

#[derive(Debug, Args)]
pub(crate) struct VerifyArgs {
    /// 解压后的发布包根目录。
    #[arg(long, value_name = "DIR")]
    pub(crate) artifact_dir: PathBuf,
    /// 已校验的外层 Release tarball SHA-256；必须等于验证回执里的 archive_sha256。
    #[arg(long, value_name = "SHA256")]
    pub(crate) package_sha256: String,
    /// `ci/release/verify-deployment-package.sh --receipt` 写出的验证回执（`auction-package-verification/1`）。
    #[arg(long, value_name = "FILE")]
    pub(crate) verification_receipt: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct MaterializeLegacyArgs {
    #[arg(long, value_enum)]
    pub(crate) surface: Surface,
    /// 旧正式二进制的回环地址（只含 scheme、主机与端口），必须绕过 CDN 与代理。
    #[arg(long, value_name = "URL")]
    pub(crate) origin_url: Url,
    /// 已验签的旧正式包解压目录；其 `docs/WEB-<SURFACE>-SHA256` 是逐文件清单的全集。
    #[arg(long, value_name = "DIR")]
    pub(crate) old_package_dir: PathBuf,
    /// 旧 build 的构建号（切流前已锚定的旧二进制身份），同时是目录名。
    #[arg(long, value_name = "BUILD_ID")]
    pub(crate) old_build_id: String,
    /// 静态挂载根目录；必须是真实目录。
    #[arg(long, value_name = "DIR")]
    pub(crate) web_root: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    #[command(flatten)]
    pub(crate) package: VerifyArgs,
    /// 服务程序的实际安装目录（绝对路径）；页面栈回执按它登记各程序的服务路径。
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: PathBuf,
    #[arg(long, value_name = "DIR")]
    pub(crate) web_root: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct SwitchArgs {
    #[arg(long, value_enum)]
    pub(crate) surface: Surface,
    /// 目标栈断言：old 必须是物化的 Dioxus build，new 必须是 Solid build。
    #[arg(long, value_enum)]
    pub(crate) to: SwitchTarget,
    /// 目标 build 的构建号。
    #[arg(long, value_name = "BUILD_ID")]
    pub(crate) build_id: String,
    #[arg(long, value_name = "DIR")]
    pub(crate) web_root: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct DrillVerifyArgs {
    /// 演练回执文件（`auction-web-switch-drill.v1`）。
    #[arg(long, value_name = "FILE")]
    pub(crate) receipt: PathBuf,
    /// 审核人签收时对回执文件计算的 SHA-256（带外给出）；不符即视为被改写。
    #[arg(long, value_name = "SHA256")]
    pub(crate) receipt_sha256: String,
    /// 静态挂载根目录；最终实物的身份与创建时间取自其 `RECEIPT.v1`。
    #[arg(long, value_name = "DIR")]
    pub(crate) web_root: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct StatusArgs {
    #[arg(long, value_name = "DIR")]
    pub(crate) web_root: PathBuf,
}
