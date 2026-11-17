//! 数据密钥环的只读核对与离线重加密。
//!
//! 重加密只改变信封的 key ID、密文、nonce 与指纹，不改变业务事实；每次写入都以旧 key ID
//! 作 CAS 条件，并在返回前用新信封重新解密核对明文。密钥材料、明文和指纹都不会进入日志、
//! 回执或审计。
//!
//! 不变量：`--apply` 已提交的改写必须留下回执与审计。回执路径、操作者、主机身份与全部域的
//! 密钥环都在连接数据库之前校验；改写中途失败时仍写 `outcome=partial` 回执、为已改写或最后
//! 一次写回结果未知的域追加审计，再返回原错误。

use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::Context;
use auction_application::core::CliOperatorIdentity;
use auction_config::DataKeyDomain;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

use self::rewrite::{DomainOutcome, FailureStage, reencrypt_domain};
use super::keyring;
use crate::app::print_json;
use crate::cli::{KeysCommand, KeysReencryptArgs};
use crate::config_dir::ConfigDir;
use crate::support::{digest, files, text};

mod rewrite;

const RECEIPT_FORMAT: &str = "auction-key-reencrypt-receipt-v1";
/// 审计的 actor_id：标明写入来自 CLI 离线重加密。
const AUDIT_ACTOR: &str = "auction-cli:keys-reencrypt";

/// 数据库信封所在域；诊断归档是一次写入文件，不在重加密范围。
const DATABASE_DOMAINS: [DataKeyDomain; 3] =
    [DataKeyDomain::Equipment, DataKeyDomain::UserLogin, DataKeyDomain::Credential];

pub(crate) async fn run(config_dir: &ConfigDir, command: KeysCommand) -> anyhow::Result<()> {
    match command {
        KeysCommand::Status { strict } => status(config_dir, strict).await,
        KeysCommand::Reencrypt(args) => reencrypt(config_dir, args).await,
    }
}

async fn status(config_dir: &ConfigDir, strict: bool) -> anyhow::Result<()> {
    super::with_worker_store(
        config_dir,
        auction_storage::DatabaseConnectionPurpose::CliOperations,
        async |config, store| {
            let pool = store.postgres_pool();
            let mut domains = serde_json::Map::new();
            let mut failures = Vec::new();
            for domain in DATABASE_DOMAINS {
                let keyring = keyring(config, domain)?;
                let stored = auction_storage::stored_key_ids(pool, domain)
                    .await
                    .with_context(|| format!("读取 {} 域信封 key ID 失败", domain.as_str()))?;
                let mut referenced = serde_json::Map::new();
                for (key_id, envelopes) in &stored {
                    let resolvable = keyring.as_ref().is_some_and(|ring| ring.contains(key_id));
                    if !resolvable {
                        failures.push(format!(
                            "{} 域仍有 {envelopes} 个信封引用未配置的 key {key_id}",
                            domain.as_str()
                        ));
                    }
                    referenced.insert(
                        key_id.clone(),
                        json!({ "envelopes": envelopes, "resolvable": resolvable }),
                    );
                }
                let now = Timestamp::now();
                let previous = config
                    .data_key_previous(domain)
                    .iter()
                    .map(|key| {
                        let still_referenced = stored.iter().any(|(id, _)| id == key.id());
                        if now >= key.retain_until() && still_referenced {
                            failures.push(format!(
                                "{} 域旧 key {} 已过 12 个月保留期但仍有信封引用；请先重加密",
                                domain.as_str(),
                                key.id()
                            ));
                        }
                        json!({
                            "key_id": key.id(),
                            "retired_at": key.retired_at().to_rfc3339(),
                            "retain_until": key.retain_until().to_rfc3339(),
                            "still_referenced": still_referenced,
                            "removable": !still_referenced && now >= key.retain_until(),
                        })
                    })
                    .collect::<Vec<_>>();
                domains.insert(
                    domain.as_str().to_owned(),
                    json!({
                        "configured": keyring.is_some(),
                        "current_key_id": keyring.as_ref().map(|ring| ring.current_id()),
                        "previous_keys": previous,
                        "database_envelopes": referenced,
                    }),
                );
            }
            // 诊断归档使用独立域，但延用 credential/equipment 的历史文件；状态只报告配置。
            let diagnostics = keyring(config, DataKeyDomain::MailDiagnostics)?;
            domains.insert(
                DataKeyDomain::MailDiagnostics.as_str().to_owned(),
                json!({
                    "configured": diagnostics.is_some(),
                    "current_key_id": diagnostics.as_ref().map(|ring| ring.current_id()),
                    "previous_keys": config
                        .data_key_previous(DataKeyDomain::MailDiagnostics)
                        .iter()
                        .map(|key| json!({
                            "key_id": key.id(),
                            "retired_at": key.retired_at().to_rfc3339(),
                            "retain_until": key.retain_until().to_rfc3339(),
                        }))
                        .collect::<Vec<_>>(),
                    "note": "诊断归档为一次写入文件，不重写；旧域 key 须保留到对应归档过期",
                }),
            );
            print_json(&json!({
                "strict": strict,
                "retention_months": auction_config::DATA_KEY_RETENTION_MONTHS,
                "domains": domains,
                "failures": failures,
            }))?;
            anyhow::ensure!(!strict || failures.is_empty(), "密钥环严格核对失败");
            Ok(())
        },
    )
    .await
}

/// 写库前的本地门禁结果。
struct ReencryptPlan<'a> {
    domains: Vec<DataKeyDomain>,
    /// `--apply` 时的审计身份；演练为 None。
    apply: Option<ApplyIdentity<'a>>,
}

/// `--apply` 的审计身份；在任何数据库访问之前取得，避免改写完成后才因环境无效而缺审计。
struct ApplyIdentity<'a> {
    actor: &'a str,
    operator: CliOperatorIdentity,
}

/// 只读本地参数的门禁：域、回执路径（绝对、父目录私有、文件尚不存在）、操作者与主机身份都在
/// 连接数据库之前校验，任何一项无效都不会改写信封。
fn plan(args: &KeysReencryptArgs) -> anyhow::Result<ReencryptPlan<'_>> {
    let domains: Vec<DataKeyDomain> = match args.domain {
        Some(arg) => vec![arg.domain()],
        None => DATABASE_DOMAINS.to_vec(),
    };
    anyhow::ensure!(
        !domains.contains(&DataKeyDomain::MailDiagnostics),
        "mail_diagnostics 归档是一次写入的诊断证据，不重写文件；请重加密数据库信封并保留旧域 key 至归档过期"
    );
    if let Some(path) = &args.receipt {
        files::require_private_target(path).context("--receipt 回执路径不可用")?;
    }
    let apply = if args.apply {
        anyhow::ensure!(args.receipt.is_some(), "--apply 必须同时提供 --receipt 回执路径");
        let actor = args.actor.as_deref().context("--apply 必须提供 --actor 审计操作者")?;
        Some(ApplyIdentity {
            actor: text::actor(actor)?,
            operator: super::operator_identity::capture()?,
        })
    } else {
        None
    };
    Ok(ReencryptPlan { domains, apply })
}

async fn reencrypt(config_dir: &ConfigDir, args: KeysReencryptArgs) -> anyhow::Result<()> {
    let plan = plan(&args)?;
    let config = config_dir.load_worker()?;
    // 全部域的密钥环在连接数据库之前装配：缺失或无效的配置不能在改写了前一个域之后才暴露。
    let mut keyrings = Vec::with_capacity(plan.domains.len());
    for domain in &plan.domains {
        let ring = keyring(&config, *domain)?
            .with_context(|| format!("{} 域缺少当前数据密钥，无法重加密", domain.as_str()))?;
        keyrings.push((*domain, ring));
    }
    super::with_config_store(
        &config,
        auction_storage::DatabaseConnectionPurpose::CliOperations,
        async |store| {
            let started_at = Timestamp::now();
            let mut processed = Vec::with_capacity(keyrings.len());
            let mut failure = None;
            for (domain, ring) in &keyrings {
                let mut outcome = DomainOutcome::default();
                let result = reencrypt_domain(
                    store,
                    ring,
                    *domain,
                    args.page_size,
                    args.apply,
                    &mut outcome,
                )
                .await;
                processed.push(ProcessedDomain {
                    domain: *domain,
                    to_key_id: ring.current_id().to_owned(),
                    outcome,
                    failure_stage: result.as_ref().err().map(|failure| failure.stage),
                });
                if let Err(error) = result {
                    failure = Some((*domain, error));
                    break;
                }
            }
            let receipt_id = Uuid::new_v4();
            let summary = failure.as_ref().map(|(domain, failure)| (*domain, failure.stage));
            let document =
                receipt_document(receipt_id, args.apply, started_at, &processed, summary);
            let Some(identity) = &plan.apply else {
                // 演练不写入任何信封：失败直接返回原因，不留回执。
                if let Some((_, failure)) = failure {
                    return Err(failure.error);
                }
                if let Some(path) = &args.receipt {
                    let encoded =
                        serde_json::to_vec_pretty(&document).context("序列化重加密回执失败")?;
                    files::write_private(path, &encoded)?;
                }
                return print_json(&document);
            };
            let recorded = record_apply(
                store,
                args.receipt.as_deref(),
                identity,
                receipt_id,
                &document,
                &processed,
            )
            .await;
            // 输出失败不能吞掉回执、审计与改写失败的原因，三者合并返回。
            let printed = print_json(&document);
            let Some((domain, failure)) = failure else {
                return recorded.and(printed);
            };
            let error = failure.error.context(format!(
                "{} 域重加密中途失败（阶段 {}）；已提交的改写数见 outcome=partial 回执与审计，\
                 修复原因后重新运行会继续处理剩余信封",
                domain.as_str(),
                failure.stage.as_str()
            ));
            let mut messages = vec![format!("{error:#}")];
            messages.extend(recorded.err().map(|record_error| format!("{record_error:#}")));
            messages.extend(printed.err().map(|print_error| format!("{print_error:#}")));
            if messages.len() == 1 {
                return Err(error);
            }
            Err(anyhow::anyhow!(messages.join("；")))
        },
    )
    .await
}

/// 一个已进入改写循环的域；`failure_stage` 为 None 表示该域处理完，否则是中途失败的阶段。
struct ProcessedDomain {
    domain: DataKeyDomain,
    to_key_id: String,
    outcome: DomainOutcome,
    failure_stage: Option<FailureStage>,
}

impl ProcessedDomain {
    const fn complete(&self) -> bool {
        self.failure_stage.is_none()
    }
}

/// 回执只含格式、标识、是否写入、结果、时间、key ID 与计数；失败只记域与固定阶段，不含
/// 错误原文、密钥材料或明文。
fn receipt_document(
    receipt_id: Uuid,
    applied: bool,
    started_at: Timestamp,
    processed: &[ProcessedDomain],
    failure: Option<(DataKeyDomain, FailureStage)>,
) -> serde_json::Value {
    let mut from_key_ids: BTreeMap<String, i64> = BTreeMap::new();
    let mut domains = serde_json::Map::new();
    for item in processed {
        for (key_id, count) in &item.outcome.from_keys {
            *from_key_ids.entry(key_id.clone()).or_default() += count;
        }
        domains.insert(
            item.domain.as_str().to_owned(),
            json!({
                "current_key_id": item.to_key_id,
                "scanned": item.outcome.scanned,
                "rewritten": item.outcome.rewritten,
                "from_key_ids": item.outcome.from_keys,
                "complete": item.complete(),
            }),
        );
    }
    json!({
        "format": RECEIPT_FORMAT,
        "receipt_id": receipt_id,
        "applied": applied,
        "outcome": if failure.is_none() { "completed" } else { "partial" },
        "failure": failure.map(|(domain, stage)| json!({
            "domain": domain.as_str(),
            "stage": stage,
        })),
        "started_at": started_at.to_rfc3339(),
        "finished_at": Timestamp::now().to_rfc3339(),
        "from_key_ids": from_key_ids,
        "domains": domains,
        "operator": { "os_uid": files::effective_uid() },
    })
}

/// 需要审计的域：完整处理过的域照常记录（含 0 次改写的幂等重跑）；中途失败的域在已有改写
/// 提交时记录，`write` 阶段失败时即使计数为 0 也记录。写回是单条自动提交的 UPDATE，服务端
/// 提交后连接中断时客户端只收到错误：那条信封已换成当前 key，重跑不会再扫描到它，此时不留
/// 审计就永远没有记录。其余没有任何改写的失败只靠回执说明。
fn audited(processed: &[ProcessedDomain]) -> impl Iterator<Item = &ProcessedDomain> {
    processed.iter().filter(|item| {
        item.complete()
            || item.outcome.rewritten > 0
            || item.failure_stage == Some(FailureStage::Write)
    })
}

/// 单个域的审计详情；`failure_stage` 为 `write` 时最后一次写回结果未知，`rewritten` 可能比
/// 实际少 1。只含 key ID、计数、阶段分类与回执身份，不含密钥材料、明文或指纹。
fn audit_details(
    item: &ProcessedDomain,
    actor: &str,
    receipt_id: Uuid,
    receipt_sha256: &str,
    receipt_written: bool,
) -> serde_json::Value {
    json!({
        "to_key_id": item.to_key_id,
        "from_key_ids": item.outcome.from_keys,
        "scanned": item.outcome.scanned,
        "rewritten": item.outcome.rewritten,
        "outcome": if item.complete() { "completed" } else { "partial" },
        "failure_stage": item.failure_stage,
        "receipt_id": receipt_id,
        "receipt_sha256": receipt_sha256,
        "receipt_written": receipt_written,
        "actor": actor,
    })
}

/// 写回执并为需要审计的域追加审计。回执写不进去时照常追加审计（已提交的改写不能没有任何
/// 记录），审计元数据注明回执未写入；两类失败合并成一个错误返回。
async fn record_apply(
    store: &Arc<DatabaseStore>,
    receipt_path: Option<&Path>,
    identity: &ApplyIdentity<'_>,
    receipt_id: Uuid,
    document: &serde_json::Value,
    processed: &[ProcessedDomain],
) -> anyhow::Result<()> {
    let encoded = serde_json::to_vec_pretty(document).context("序列化重加密回执失败")?;
    let mut problems = Vec::new();
    let receipt_written = match receipt_path {
        Some(path) => match files::write_private(path, &encoded) {
            Ok(()) => true,
            Err(error) => {
                problems.push(format!("重加密回执未写入：{error:#}"));
                false
            }
        },
        None => false,
    };
    let receipt_sha256 = digest::sha256_bytes(&encoded);
    for item in audited(processed) {
        let metadata = auction_application::core::cli_audit_metadata(
            Some(&identity.operator),
            audit_details(item, identity.actor, receipt_id, &receipt_sha256, receipt_written),
        );
        if let Err(error) = auction_storage::append_reencrypt_audit(
            store.postgres_pool(),
            item.domain,
            AUDIT_ACTOR,
            metadata,
        )
        .await
        {
            problems.push(format!(
                "追加 {} 域重加密审计失败：{:#}",
                item.domain.as_str(),
                anyhow::Error::from(error)
            ));
        }
    }
    if problems.is_empty() { Ok(()) } else { anyhow::bail!("{}", problems.join("；")) }
}

#[cfg(test)]
mod tests;
