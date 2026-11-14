//! 单文件服务的显式装配参数；不通过环境变量选择角色配置。

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(name = "auction-server", version, about = "Auction 单文件三角色服务")]
pub(super) struct Cli {
    /// 三角色配置目录；不存在时进入首次设置，已有目录绝不覆盖。
    #[arg(long, default_value = "auction-config")]
    pub config_dir: PathBuf,
    /// 初始化监听器，默认仅本机可访问。
    #[arg(long, default_value = "127.0.0.1:8091")]
    pub setup_listen: SocketAddr,
    /// 显式允许受信 HTTPS 代理访问首次设置，不开放普通 Admin 的 setup API。
    #[arg(long, requires_all = ["setup_origin", "setup_proxy_ip"])]
    pub allow_remote_setup: bool,
    /// 反向代理对外的精确 HTTPS origin，不含路径。
    #[arg(long, requires = "allow_remote_setup")]
    pub setup_origin: Option<String>,
    /// 唯一受信代理的 TCP 源地址；转发头只有来自它才有效。
    #[arg(long, requires = "allow_remote_setup")]
    pub setup_proxy_ip: Option<IpAddr>,
    /// 首次设置初始化空库使用的迁移 owner 凭据；已有配置目录时忽略。
    /// 服务账号必须是无 DDL 的 auction_runtime，因此首次迁移不能复用服务连接。
    #[arg(long, value_name = "FILE")]
    pub owner_secrets: Option<PathBuf>,
    /// 每个角色等待 readiness 的秒数。
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub startup_timeout_seconds: u64,
    #[arg(long, hide = true, value_enum, requires = "config_file")]
    pub role: Option<ChildRole>,
    #[arg(long, hide = true, requires = "role")]
    pub config_file: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum ChildRole {
    Worker,
    Admin,
    Public,
}

impl ChildRole {
    pub const ORDER: [Self; 3] = [Self::Worker, Self::Admin, Self::Public];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }
    pub const fn config_role(self) -> auction_config::Role {
        match self {
            Self::Worker => auction_config::Role::Worker,
            Self::Admin => auction_config::Role::Admin,
            Self::Public => auction_config::Role::Public,
        }
    }
}
