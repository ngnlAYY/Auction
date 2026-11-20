//! 在加载任何角色配置前选择正常启动或本机首次初始化。

use std::{
    env,
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

#[cfg(feature = "web-embed")]
use crate::web_assets::web_script_policy;

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
            bail!("admin-api 不支持该参数；只能使用 `setup` 或不带参数");
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
        output_dir: env::current_dir().context("定位首次设置输出目录失败")?.join("auction-config"),
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
        Err(error) => Err(error).context("无法检查 Admin 配置路径"),
    }
}

fn parse_setup(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<StartMode> {
    let mut output_dir = None;
    let mut control_group = "auction-worker-control".to_owned();
    // SAFETY：只读取当前进程的有效 UID，不解引用外部指针。
    let mut control_uid = unsafe { libc::geteuid() };
    while let Some(option) = args.next() {
        let value = args.next().context("设置选项缺少取值")?;
        match option.to_str() {
            Some("--output-dir") => output_dir = Some(PathBuf::from(value)),
            Some("--control-group") => {
                control_group =
                    value.into_string().map_err(|_| anyhow::anyhow!("control 用户组无效"))?;
            }
            Some("--control-uid") => {
                control_uid = value
                    .to_str()
                    .context("control UID 无效")?
                    .parse()
                    .context("control UID 无效")?;
            }
            _ => bail!("不支持的设置选项"),
        }
    }
    let output_dir = match output_dir {
        Some(path) => path,
        None => env::current_dir().context("定位首次设置输出目录失败")?.join("auction-config"),
    };
    if !output_dir.is_absolute() {
        bail!("设置输出目录必须是绝对路径");
    }
    Ok(StartMode::Setup(SetupOptions { output_dir, control_group, control_uid }))
}

pub(super) async fn run(options: SetupOptions) -> anyhow::Result<()> {
    auction_observability::init_json_stderr("off,auction::panic=error")
        .context("初始化首次设置日志失败")?;
    auction_observability::install_safe_panic_hook();
    #[cfg(feature = "web-embed")]
    {
        auction_setup::run_first_run::<crate::web_assets::AdminWebAssets>(
            auction_setup::FirstRunServerOptions {
                output_dir: options.output_dir,
                control_group: options.control_group,
                control_uid: options.control_uid,
            },
            web_script_policy(),
        )
        .await
        .map_err(|reason| anyhow::anyhow!(reason))
    }
    #[cfg(not(feature = "web-embed"))]
    {
        let _ = (options.output_dir, options.control_group, options.control_uid);
        bail!("首次设置需要启用 web-embed 的 PostgreSQL 发布构建");
    }
}

#[cfg(test)]
#[cfg(feature = "web-embed")]
mod tests {
    use std::{fs, io::ErrorKind, path::Path, time::Duration};

    use auction_setup::FirstRunPolicy;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        time::{sleep, timeout},
    };

    use super::{SetupOptions, run};

    #[tokio::test]
    #[ignore = "需要已构建的 Admin 入口与空闲的回环端口 8091"]
    async fn split_setup_serves_the_selected_admin_web_stack() -> anyhow::Result<()> {
        let (directory, script_src) = if cfg!(feature = "web-solid") {
            ("target/web/admin/public", "script-src 'self'")
        } else {
            (
                "target/dx/build/auction-web-admin/web/public",
                "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'",
            )
        };
        let index =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(directory).join("index.html");
        let expected = fs::read(index).expect("请先构建所选技术栈的 Admin 入口");
        let options = SetupOptions {
            output_dir: std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
            control_group: "auction".to_owned(),
            control_uid: 1000,
        };
        let address = FirstRunPolicy::default().listen;
        let request = async {
            let mut stream = loop {
                match TcpStream::connect(address).await {
                    Ok(stream) => break stream,
                    Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                        sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("无法连接首次设置监听器：{error}"),
                }
            };
            let request =
                format!("GET /setup HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        };
        let response = tokio::select! {
            biased;
            result = run(options) => {
                result?;
                anyhow::bail!("首次设置在响应前退出");
            }
            response = timeout(Duration::from_secs(5), request) => response.expect("读取设置页超时"),
        };
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
        assert_eq!(body.as_bytes(), expected);
        assert!(
            headers.contains(&format!(
                "\r\ncontent-security-policy: default-src 'self'; {script_src};"
            )),
            "{headers}"
        );
        Ok(())
    }
}
