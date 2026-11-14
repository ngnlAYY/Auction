//! 在加载任何角色配置前选择正常启动或本机首次初始化。

use std::{
    env,
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

pub(super) enum StartMode {
    Normal(Option<PathBuf>),
    Setup(SetupOptions),
}

pub(super) struct SetupOptions {
    output_dir: PathBuf,
    control_group: String,
    control_uid: u32,
}

pub(super) fn select_mode() -> anyhow::Result<StartMode> {
    let mut args = env::args_os().skip(1);
    if let Some(command) = args.next() {
        if command != "setup" {
            bail!("unsupported admin-api argument; use `setup` or no arguments");
        }
        return parse_setup(args);
    }
    if env::var_os("AUCTION_CONFIG_FILE").is_some()
        || config_entry_exists(Path::new("ops/config/admin.toml"))?
    {
        return Ok(StartMode::Normal(None));
    }
    let local = PathBuf::from("auction-config/admin.toml");
    if config_entry_exists(&local)? {
        return Ok(StartMode::Normal(Some(local)));
    }
    Ok(StartMode::Setup(SetupOptions {
        output_dir: env::current_dir()
            .context("locate first-run output directory")?
            .join("auction-config"),
        control_group: "auction-worker-control".to_owned(),
        // SAFETY：只读取当前进程的有效 UID，不解引用外部指针。
        control_uid: unsafe { libc::geteuid() },
    }))
}

/// 只有明确不存在才能进入首次设置；目录、悬空链接等现有条目交给正式 loader 拒绝。
fn config_entry_exists(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("cannot inspect admin configuration path"),
    }
}

fn parse_setup(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<StartMode> {
    let mut output_dir = None;
    let mut control_group = "auction-worker-control".to_owned();
    // SAFETY：只读取当前进程的有效 UID，不解引用外部指针。
    let mut control_uid = unsafe { libc::geteuid() };
    while let Some(option) = args.next() {
        let value = args.next().context("setup option is missing a value")?;
        match option.to_str() {
            Some("--output-dir") => output_dir = Some(PathBuf::from(value)),
            Some("--control-group") => {
                control_group =
                    value.into_string().map_err(|_| anyhow::anyhow!("invalid control group"))?;
            }
            Some("--control-uid") => {
                control_uid = value
                    .to_str()
                    .context("invalid control UID")?
                    .parse()
                    .context("invalid control UID")?;
            }
            _ => bail!("unsupported setup option"),
        }
    }
    let output_dir = match output_dir {
        Some(path) => path,
        None => {
            env::current_dir().context("locate first-run output directory")?.join("auction-config")
        }
    };
    if !output_dir.is_absolute() {
        bail!("setup output directory must be absolute");
    }
    Ok(StartMode::Setup(SetupOptions { output_dir, control_group, control_uid }))
}

pub(super) async fn run(options: SetupOptions) -> anyhow::Result<()> {
    auction_observability::init_json_stderr("off,auction::panic=error")
        .context("initialize setup tracing")?;
    auction_observability::install_safe_panic_hook();
    #[cfg(feature = "web-embed")]
    {
        auction_setup::run_first_run::<crate::web_assets::AdminWebAssets>(
            auction_setup::FirstRunServerOptions {
                output_dir: options.output_dir,
                control_group: options.control_group,
                control_uid: options.control_uid,
            },
        )
        .await
        .map_err(|reason| anyhow::anyhow!(reason))
    }
    #[cfg(not(feature = "web-embed"))]
    {
        let _ = (options.output_dir, options.control_group, options.control_uid);
        bail!("first-run setup requires a PostgreSQL web-embed binary");
    }
}
