//! 规则目录首次导入：先完整读取五种语言并核对数据库现状，任何已发布的不同正文都在写入前拒绝；
//! 缺失的语言以稳定派生的命令 ID 发布，部分完成后用同一批次 ID 重试即可续上。

use std::{fs, path::Path, sync::Arc};

use anyhow::Context;
use auction_application::admin::{
    AdminRulesCandidate, AdminRulesPort, AdminRulesPublishCommand, AdminRulesRequest,
    MAX_RULE_MARKDOWN_BYTES,
};
use auction_application::core::{RequestHash, lower_hex};
use auction_application::public::PUBLIC_RULES_LOCALES;
use auction_contracts::AdminRulesPublishRequest;
use auction_storage::DatabaseAdminRulesCommands;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::RulesCommand;
use crate::config_dir::ConfigDir;
use crate::support::{files, text};

/// 与 Admin 发布接口相同的请求种类，保证同一正文在两个入口得到同一请求摘要。
const REQUEST_KIND: &str = "admin_rules_publish";
/// 单个 UTF-8 BOM 的字节数；读取预算允许正文上限加一个 BOM。
const BOM_BYTES: usize = 3;

struct RulesSource {
    locale: String,
    title: String,
    markdown: String,
    sha256: String,
}

#[derive(Serialize)]
struct DocumentReport {
    locale: String,
    action: &'static str,
    revision: i64,
    sha256: String,
    command_id: Option<Uuid>,
}

pub(crate) async fn run(config_dir: &ConfigDir, command: RulesCommand) -> anyhow::Result<()> {
    let RulesCommand::Import { directory, command_id, actor } = command;
    let command_id = text::command_id(command_id, "--command-id")?;
    let actor = text::actor(&actor)?.to_owned();
    let directory = fs::canonicalize(&directory).context("解析规则目录失败")?;
    if !directory.is_dir() {
        anyhow::bail!("--directory 必须是现有目录");
    }
    let sources = PUBLIC_RULES_LOCALES
        .iter()
        .map(|locale| load_source(&directory, locale))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let (_, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = async {
        let rules =
            DatabaseAdminRulesCommands::new(Arc::clone(&store), Arc::new(super::SystemClock));
        import(&rules, sources, command_id, &actor).await
    }
    .await;
    store.postgres_pool().close().await;
    let documents = result?;
    print_json(&serde_json::json!({
        "command_id": command_id,
        "documents": documents,
    }))
}

async fn import(
    rules: &dyn AdminRulesPort,
    sources: Vec<RulesSource>,
    command_id: Uuid,
    actor: &str,
) -> anyhow::Result<Vec<DocumentReport>> {
    // 写入前核对全部语言；不同正文意味着 Admin 已有权威内容，导入不得覆盖。
    let mut current = Vec::with_capacity(sources.len());
    for source in &sources {
        let document = rules
            .get(AdminRulesRequest { locale: source.locale.clone() })
            .await
            .with_context(|| format!("读取 {} 当前规则失败", source.locale))?;
        if let Some(document) = &document
            && (document.title != source.title || document.markdown != source.markdown)
        {
            anyhow::bail!("数据库中的 {} 规则与目录不同；导入已在任何写入前停止", source.locale);
        }
        current.push(document);
    }
    let request_id = format!("auction-cli:rules-import:{command_id}");
    let mut documents = Vec::with_capacity(sources.len());
    for (source, existing) in sources.into_iter().zip(current) {
        if let Some(document) = existing {
            documents.push(DocumentReport {
                locale: source.locale,
                action: "preserved",
                revision: document.revision,
                sha256: source.sha256,
                command_id: None,
            });
            continue;
        }
        let child =
            Uuid::new_v5(&command_id, format!("rules-directory:{}", source.locale).as_bytes());
        let request = AdminRulesPublishRequest {
            command_id: child,
            expected_revision: 0,
            locale: source.locale.clone(),
            title: source.title,
            markdown: source.markdown,
        };
        let request_hash = RequestHash::from_request(REQUEST_KIND, child, &request)?;
        let document = rules
            .publish(AdminRulesPublishCommand {
                command_id: child.into(),
                expected_revision: request.expected_revision,
                locale: request.locale,
                title: request.title,
                markdown: request.markdown,
                actor_id: actor.to_owned(),
                operator_identity: Some(super::operator_identity::capture()?),
                request_id: Some(request_id.clone()),
                request_hash,
            })
            .await
            .with_context(|| format!("发布 {} 规则失败", source.locale))?;
        documents.push(DocumentReport {
            locale: source.locale,
            action: "imported",
            revision: document.revision,
            sha256: source.sha256,
            command_id: Some(child),
        });
    }
    Ok(documents)
}

/// 读取 `<locale>.md`：去掉开头单个 BOM，首个非空行去掉前导 `#` 作为标题，正文规范化后校验。
fn load_source(directory: &Path, locale: &str) -> anyhow::Result<RulesSource> {
    let path = directory.join(format!("{locale}.md"));
    let limit = (MAX_RULE_MARKDOWN_BYTES + BOM_BYTES) as u64;
    let bytes =
        files::read_bounded(&path, limit).with_context(|| format!("读取 {locale}.md 失败"))?;
    let mut markdown =
        String::from_utf8(bytes).with_context(|| format!("{locale}.md 不是 UTF-8"))?;
    if markdown.starts_with('\u{feff}') {
        markdown.drain(..'\u{feff}'.len_utf8());
    }
    let title = markdown
        .split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_owned())
        .filter(|title| !title.is_empty())
        .with_context(|| format!("{locale}.md 的首个非空行不能解析为标题"))?;
    let candidate = AdminRulesCandidate { locale: locale.to_owned(), title, markdown }.normalize();
    candidate.validate().with_context(|| format!("{locale}.md 未通过规则校验"))?;
    let sha256 = lower_hex(&Sha256::digest(candidate.markdown.as_bytes()));
    Ok(RulesSource {
        locale: candidate.locale,
        title: candidate.title,
        markdown: candidate.markdown,
        sha256,
    })
}
