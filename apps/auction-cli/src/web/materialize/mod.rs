//! `auction-cli web verify | materialize-legacy | install | switch | status`：页面静态挂载的物化与切换。
//!
//! 首次 Rust → Solid 采用时，旧 Dioxus 壳的字节从旧正式二进制的回环源站按旧包清单逐文件取回、
//! 全部核对通过后才安装成只读 build（`<web_root>/<surface>/<build_id>/`）；入口指针只由 `switch`
//! 与首次 `install` 原子写入，页面栈回执 `RECEIPT.v1` 随之重写。命令不连接数据库、不停服务；
//! 服务启动时一次性读取挂载与回执，切换后须重启 Public/Admin。
//!
//! 旧页面文件按原相对路径（含带指纹的原 URL）存放在 build 的 `files/` 下，服务按 URL 精确查表，
//! 因此“旧页面实际使用的原指纹 URL 别名”由存放路径本身给出，不需要另建别名。

mod export;
mod fetch;
mod install;
mod layout;
mod package;
mod payload;
mod pointer;
mod receipt;
#[cfg(test)]
mod tests;
mod verify;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use auction_integrations::{WebFetchClient, WebFetchPolicy};
use auction_time::Timestamp;
use serde_json::json;

use self::install::{InstallFile, Installed, NewBuild, ensure_surface_root, install_build};
use self::layout::{Stack, validate_build_id};
use self::package::verify_package;
use self::payload::PayloadList;
use self::receipt::{BinaryRecord, PackageRecord};
use crate::app::{print_json, progress};
use crate::cli::{InstallArgs, MaterializeLegacyArgs, SwitchArgs, VerifyArgs, WebCommand};
use crate::deploy::artifact::{self, WebStackIdentity};
use crate::support::files;

pub(crate) use self::layout::Surface;
pub(crate) use self::pointer::SwitchTarget;

/// 页面摘要清单自身的读取上限。
const MAX_LIST_BYTES: u64 = 4 << 20;

pub(crate) async fn run(command: WebCommand) -> anyhow::Result<()> {
    match command {
        WebCommand::Verify(args) => verify(&args),
        WebCommand::MaterializeLegacy(args) => materialize_legacy(&args).await,
        WebCommand::Install(args) => install_new(&args),
        WebCommand::Switch(args) => switch(&args),
        WebCommand::Status(args) => print_json(&pointer::status(&args.web_root)?),
        WebCommand::DrillVerify(args) => {
            crate::web::drill::run(&args.receipt, &args.receipt_sha256, &args.web_root)
        }
    }
}

fn verify(args: &VerifyArgs) -> anyhow::Result<()> {
    let verified = verify_package(args, Timestamp::now())?;
    print_json(&json!({
        "package": verified.artifact.package,
        "source_commit": verified.artifact.source_commit,
        "tag": verified.tag,
        "archive_sha256": verified.archive_sha256,
        "web_stack": verified.artifact.web_stack.as_ref().map(WebStackIdentity::projection),
        "verified_at": verified.verified_at.to_rfc3339(),
    }))
}

/// 旧 Dioxus 壳物化：旧包验过 → 逐文件取回并核对 → 安装只读 build；不改入口指针。
async fn materialize_legacy(args: &MaterializeLegacyArgs) -> anyhow::Result<()> {
    let list = read_legacy_list(&args.old_package_dir, args.surface)?;
    progress(format!(
        "从 {} 取回旧 {} 页面 {} 个文件",
        args.origin_url,
        args.surface.as_str(),
        list.files.len()
    ));
    let outcome = materialize(
        &args.web_root,
        args.surface,
        &args.old_build_id,
        &args.origin_url,
        &list,
        WebFetchPolicy::default(),
    )
    .await?;
    print_json(&json!({
        "surface": args.surface.as_str(),
        "build_id": args.old_build_id,
        "stack": Stack::Dioxus.as_str(),
        "payload_sha256": list.payload_sha256,
        "files": list.files.len(),
        "installed": matches!(outcome, Installed::Created),
        "pointer_changed": false,
    }))
}

/// 全部取回并核对通过之后才写盘：任何一个文件不符都在安装之前失败，磁盘与指针保持原样。
async fn materialize(
    web_root: &Path,
    surface: Surface,
    old_build_id: &str,
    origin: &url::Url,
    list: &PayloadList,
    policy: WebFetchPolicy,
) -> anyhow::Result<Installed> {
    validate_build_id(old_build_id)?;
    let client = WebFetchClient::new(origin, policy)?;
    let files: Vec<InstallFile> = fetch::fetch_all(&client, list).await?;
    let build = NewBuild {
        surface,
        build_id: old_build_id.to_owned(),
        stack: Stack::Dioxus,
        payload_sha256: list.payload_sha256.clone(),
        files,
    };
    install_build(web_root, &build)
}

/// 旧包必须先经制品校验（来源摘要、成员与权限），且是旧格式；摘要清单的摘要与包校验结果一致。
fn read_legacy_list(old_package_dir: &Path, surface: Surface) -> anyhow::Result<PayloadList> {
    let package = artifact::inspect(old_package_dir).context("旧正式包校验失败")?;
    if package.web_stack.is_some() {
        anyhow::bail!("旧壳必须来自旧格式（Rust 版）正式包，不是新格式的 Solid 包");
    }
    let list_path = old_package_dir.join(format!("docs/WEB-{}-SHA256", surface.upper()));
    let list = PayloadList::parse(&files::read_bounded(&list_path, MAX_LIST_BYTES)?)?;
    let declared = match surface {
        Surface::Public => &package.public_payload_sha256,
        Surface::Admin => &package.admin_payload_sha256,
    };
    if list.payload_sha256 != *declared {
        anyhow::bail!("旧包的页面摘要清单与包校验结果不一致：{}", surface.as_str());
    }
    Ok(list)
}

/// 把新版正式包的两端导出树装成只读 build；两端都读取并核对完才开始写入。首次安装时
/// 建立入口指针（默认与独立入口都是新 build），已有指针则保持不变，须用 `switch` 换入。
/// 旧壳已物化时同时写页面栈回执；否则不写回执（启用校验随后失败关闭）。
fn install_new(args: &InstallArgs) -> anyhow::Result<()> {
    if !args.install_dir.is_absolute() {
        anyhow::bail!("--install-dir 必须是绝对路径");
    }
    let verified = verify_package(&args.package, Timestamp::now())?;
    let identity =
        verified.artifact.web_stack.as_ref().context("发布包没有页面凭据 docs/WEB-STACK.v1")?;
    let mut builds = Vec::new();
    for (surface, payload_sha256) in [
        (Surface::Public, &identity.public_payload_sha256),
        (Surface::Admin, &identity.admin_payload_sha256),
    ] {
        let list_path =
            args.package.artifact_dir.join(format!("docs/WEB-{}-SHA256", surface.upper()));
        let list = PayloadList::parse(&files::read_bounded(&list_path, MAX_LIST_BYTES)?)?;
        builds.push(export::read_export_tree(
            &args.package.artifact_dir,
            surface,
            &list,
            &identity.build_id,
            payload_sha256,
        )?);
    }
    let mut report = Vec::new();
    for build in &builds {
        let outcome = install_build(&args.web_root, build)?;
        let surface_root = ensure_surface_root(&args.web_root, build.surface)?;
        let pointer_created = pointer::init_pointer(&surface_root, build.surface, &build.build_id)?;
        report.push(json!({
            "surface": build.surface.as_str(),
            "build_id": build.build_id,
            "files": build.files.len(),
            "installed": matches!(outcome, Installed::Created),
            "pointer_created": pointer_created,
        }));
    }
    // 同一份实物重复 install 不刷新首次安装时刻；换了实物（或旧格式回执无法解析）才取当前时刻。
    let installed_at = match receipt::read_package(&args.web_root) {
        Ok(Some(previous)) if previous.archive_sha256 == verified.archive_sha256 => {
            previous.installed_at
        }
        _ => Timestamp::now(),
    };
    let package = PackageRecord {
        installed_at,
        package: verified.artifact.package.clone(),
        source_commit: verified.artifact.source_commit.clone(),
        tag: verified.tag.clone(),
        archive_sha256: verified.archive_sha256.clone(),
        binaries: binaries_of(&verified.artifact, &args.install_dir),
    };
    let receipt = match receipt::render(&package, &args.web_root, None, Timestamp::now()) {
        Ok(value) => {
            receipt::write(&args.web_root, &value)?;
            json!({ "written": true })
        }
        // 缺旧壳等：不写回执，启用校验会因缺回执失败关闭；安装本身已完成。
        Err(error) => json!({ "written": false, "reason": format!("{error:#}") }),
    };
    print_json(&json!({ "builds": report, "receipt": receipt }))
}

/// 回执登记的程序：分体包的四个程序按实际服务路径与已校验摘要；`auction-server` 登记为空。
fn binaries_of(
    package: &artifact::Artifact,
    install_dir: &Path,
) -> BTreeMap<String, Option<BinaryRecord>> {
    let mut binaries: BTreeMap<String, Option<BinaryRecord>> = package
        .binaries
        .iter()
        .map(|binary| {
            let record = BinaryRecord {
                path: install_dir.join(binary.name).display().to_string(),
                sha256: binary.sha256.clone(),
            };
            (binary.name.to_owned(), Some(record))
        })
        .collect();
    binaries.insert("auction-server".to_owned(), None);
    binaries
}

/// 切换入口指针并重写回执。写盘顺序保证回执从不声称“Solid 入口”而指针实际在旧壳：
/// 回切旧壳先写回执（启用校验随即拒绝）再换指针，换入 Solid 先换指针再写回执。
fn switch(args: &SwitchArgs) -> anyhow::Result<()> {
    let surface_root = args.web_root.join(args.surface.as_str());
    let next = pointer::plan_switch(&args.web_root, args.surface, args.to, &args.build_id)?;
    let receipt = match receipt::read_package(&args.web_root)? {
        Some(package) => Some(receipt::render(
            &package,
            &args.web_root,
            Some((args.surface, &next)),
            Timestamp::now(),
        )?),
        None => None,
    };
    let write_receipt =
        || receipt.as_ref().map_or(Ok(()), |value| receipt::write(&args.web_root, value));
    match args.to {
        SwitchTarget::Old => {
            write_receipt()?;
            pointer::write_pointer(&surface_root, &next)?;
        }
        SwitchTarget::New => {
            pointer::write_pointer(&surface_root, &next)?;
            write_receipt()?;
        }
    }
    progress("入口指针已原子替换；Public/Admin 须重启后生效");
    print_json(&json!({
        "surface": args.surface.as_str(),
        "build_id": next.build_id,
        "stack": next.stack.as_str(),
        "independent_build_id": next.independent_build_id,
        "receipt_rewritten": receipt.is_some(),
    }))
}
