//! Auction-BF 规则目录到 PostgreSQL 权威文档的显式导入命令。

use std::path::Path;

use uuid::Uuid;

#[cfg(feature = "postgres")]
use std::{fmt::Write as _, fs, io::Read as _, os::unix::fs::OpenOptionsExt as _, sync::Arc};

#[cfg(feature = "postgres")]
use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminRulesCandidate, AdminRulesPort, AdminRulesPublishCommand, AdminRulesRequest, Clock,
    MAX_RULE_ACTOR_BYTES, MAX_RULE_MARKDOWN_BYTES, PUBLIC_RULES_LOCALES, RequestHash,
    public_rules_cache_key,
};
#[cfg(feature = "postgres")]
use auction_cache::CacheStore;
#[cfg(feature = "postgres")]
use auction_config::{AppConfig, CacheBackend, Role};
#[cfg(feature = "postgres")]
use auction_contracts::AdminRulesPublishRequest;
#[cfg(feature = "postgres")]
use auction_storage::{DatabaseAdminRulesCommands, DatabaseBackend, DatabaseStore};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde::Serialize;
#[cfg(feature = "postgres")]
use sha2::{Digest, Sha256};

#[cfg(feature = "postgres")]
const IMPORT_REQUEST_KIND: &str = "admin_rules_publish";

#[cfg(feature = "postgres")]
#[derive(Clone, Copy)]
struct RulesImportClock;

#[cfg(feature = "postgres")]
impl Clock for RulesImportClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(feature = "postgres")]
struct RulesSource {
    locale: String,
    title: String,
    markdown: String,
    sha256: String,
}

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct RulesImportDocumentReport {
    locale: String,
    action: &'static str,
    revision: i64,
    sha256: String,
    command_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
#[derive(Serialize)]
struct RulesImportReport {
    command_id: Uuid,
    documents: Vec<RulesImportDocumentReport>,
    cache_invalidation_failures: Vec<String>,
}

#[cfg(feature = "postgres")]
pub(crate) async fn import_rules_directory(
    directory: &Path,
    command_id: Uuid,
    actor: &str,
) -> anyhow::Result<()> {
    validate_import_identity(command_id, actor)?;
    let directory = fs::canonicalize(directory).context("resolve rules directory")?;
    if !directory.is_dir() {
        anyhow::bail!("rules directory 必须是现有目录");
    }

    // 先完整读取五种语言，避免缺文件或损坏文件造成可预防的部分导入。
    let sources = PUBLIC_RULES_LOCALES
        .iter()
        .map(|locale| load_rules_source(&directory, locale))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let config =
        AppConfig::load(Role::Admin).context("load admin configuration for rules import")?;
    let cache = configure_cache(&config).context("configure rules cache invalidation")?;
    let database_url =
        config.database_url().context("database_url is required for rules import")?;
    if DatabaseBackend::from_url(database_url)? != DatabaseBackend::Postgres {
        anyhow::bail!("import-rules-directory 只支持 PostgreSQL 完整档");
    }
    let store = Arc::new(
        DatabaseStore::connect(database_url).await.context("connect database for rules import")?,
    );
    let rules = DatabaseAdminRulesCommands::new(store, Arc::new(RulesImportClock));

    // 在任何写入前核对全部现有语言；不同正文意味着 Admin 已有权威内容，导入不得覆盖。
    let mut current_documents = Vec::with_capacity(sources.len());
    for source in &sources {
        let current = rules
            .get(AdminRulesRequest { locale: source.locale.clone() })
            .await
            .with_context(|| format!("read current rules for {}", source.locale))?;
        if let Some(document) = current.as_ref()
            && (document.title != source.title || document.markdown != source.markdown)
        {
            anyhow::bail!(
                "数据库中的 {} 规则与目录不同；为避免覆盖已发布内容，导入已在写入前停止",
                source.locale
            );
        }
        current_documents.push(current);
    }

    let request_id = format!("auctionctl:rules-import:{command_id}");
    let mut documents = Vec::with_capacity(sources.len());
    for (source, current) in sources.into_iter().zip(current_documents) {
        if let Some(document) = current {
            documents.push(RulesImportDocumentReport {
                locale: source.locale,
                action: "preserved",
                revision: document.revision,
                sha256: source.sha256,
                command_id: None,
            });
            continue;
        }

        let child_command_id = rules_command_id(command_id, &source.locale);
        let request = AdminRulesPublishRequest {
            command_id: child_command_id,
            expected_revision: 0,
            locale: source.locale.clone(),
            title: source.title,
            markdown: source.markdown,
        };
        let request_hash =
            RequestHash::from_request(IMPORT_REQUEST_KIND, child_command_id, &request)
                .with_context(|| format!("build rules request digest for {}", source.locale))?;
        let document = rules
            .publish(AdminRulesPublishCommand {
                command_id: child_command_id.into(),
                expected_revision: request.expected_revision,
                locale: request.locale,
                title: request.title,
                markdown: request.markdown,
                actor_id: actor.to_owned(),
                request_id: Some(request_id.clone()),
                request_hash,
            })
            .await
            .with_context(|| format!("publish imported rules for {}", source.locale))?;
        documents.push(RulesImportDocumentReport {
            locale: source.locale,
            action: "imported",
            revision: document.revision,
            sha256: source.sha256,
            command_id: Some(child_command_id),
        });
    }

    // 缓存不是权威状态；失效失败不回滚已提交发布，但必须进入机器可读结果供操作员处置。
    let mut cache_invalidation_failures = Vec::new();
    for document in &documents {
        let Some(key) = public_rules_cache_key(&document.locale) else {
            continue;
        };
        if cache.invalidate(&key).await.is_err() {
            cache_invalidation_failures.push(document.locale.clone());
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&RulesImportReport {
            command_id,
            documents,
            cache_invalidation_failures,
        })?
    );
    Ok(())
}

#[cfg(feature = "postgres")]
fn validate_import_identity(command_id: Uuid, actor: &str) -> anyhow::Result<()> {
    if command_id.is_nil() {
        anyhow::bail!("command-id 必须是非零 UUID");
    }
    if actor.is_empty()
        || actor != actor.trim()
        || actor.len() > MAX_RULE_ACTOR_BYTES
        || actor.chars().any(char::is_control)
    {
        anyhow::bail!("actor 必须是 1 至 {} 字节的规范非控制字符文本", MAX_RULE_ACTOR_BYTES);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn load_rules_source(directory: &Path, locale: &str) -> anyhow::Result<RulesSource> {
    let path = directory.join(format!("{locale}.md"));
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("open rules file {locale}.md"))?;
    let metadata = file.metadata().with_context(|| format!("inspect rules file {locale}.md"))?;
    if !metadata.is_file() {
        anyhow::bail!("规则文件 {locale}.md 必须是普通文件且不能是符号链接");
    }
    if metadata.len() > MAX_RULE_MARKDOWN_BYTES as u64 + 3 {
        anyhow::bail!("规则文件 {locale}.md 超过允许大小");
    }
    // 额外读取一个字节识别并发增长；预算仍允许原有正文上限加单个 UTF-8 BOM。
    let mut bytes = Vec::new();
    file.take(MAX_RULE_MARKDOWN_BYTES as u64 + 4)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read rules file {locale}.md"))?;
    if bytes.len() > MAX_RULE_MARKDOWN_BYTES + 3 {
        anyhow::bail!("规则文件 {locale}.md 超过允许大小");
    }
    let mut markdown =
        String::from_utf8(bytes).with_context(|| format!("规则文件 {locale}.md 不是 UTF-8"))?;
    // Auction-BF 会移除文件开头的单个 UTF-8 BOM，并保持其余 Markdown 字节语义不变。
    if markdown.starts_with('\u{feff}') {
        drop(markdown.drain(..'\u{feff}'.len_utf8()));
    }
    let first_nonblank = markdown
        .split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
        .with_context(|| format!("规则文件 {locale}.md 没有标题"))?;
    let title = first_nonblank.trim_start_matches('#').trim().to_owned();
    if title.is_empty() {
        anyhow::bail!("规则文件 {locale}.md 的首个非空行不能解析为标题");
    }
    let candidate = AdminRulesCandidate { locale: locale.to_owned(), title, markdown }.normalize();
    candidate.validate().with_context(|| format!("validate rules file {locale}.md"))?;
    let mut sha256 = String::with_capacity(64);
    // 保持导入回执原有的 64 位小写摘要；只适配 digest 0.11 的输出数组类型。
    for byte in Sha256::digest(candidate.markdown.as_bytes()) {
        let _ = write!(sha256, "{byte:02x}");
    }
    Ok(RulesSource {
        locale: candidate.locale,
        title: candidate.title,
        markdown: candidate.markdown,
        sha256,
    })
}

#[cfg(feature = "postgres")]
fn rules_command_id(batch_command_id: Uuid, locale: &str) -> Uuid {
    Uuid::new_v5(&batch_command_id, format!("rules-directory:{locale}").as_bytes())
}

#[cfg(feature = "postgres")]
fn configure_cache(config: &AppConfig) -> anyhow::Result<CacheStore> {
    match config.cache.backend {
        CacheBackend::Disabled => Ok(CacheStore::disabled()),
        CacheBackend::Redis => {
            let redis_url = config.redis_url().context("redis cache URL is missing")?;
            CacheStore::redis(redis_url).map_err(Into::into)
        }
    }
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn import_rules_directory(
    _directory: &Path,
    _command_id: Uuid,
    _actor: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("import-rules-directory 需要 PostgreSQL 完整档；请使用 --features postgres")
}
