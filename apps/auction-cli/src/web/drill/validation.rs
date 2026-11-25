//! 页面切换演练回执逐段验证：身份、实物绑定、逐响应策略、人工签收。

use std::collections::{BTreeMap, BTreeSet};

use auction_time::Timestamp;
use serde::Deserialize;

use super::{DrillEvidence, FORMAT, Requirements};
use crate::support::{
    digest::{is_lower_hex, sha256_bytes},
    text,
};

/// 旧 Dioxus 页面的 CSP；与 `auction-http` 的私有常量逐字节相同，测试对照真实安全头防漂移。
pub(super) const DIOXUS_CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "object-src 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'; ",
    "frame-ancestors 'none'",
);

/// 新版 Solid 页面的 CSP；同上。
pub(super) const SOLID_CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "object-src 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'; ",
    "frame-ancestors 'none'",
);

/// 演练的三个阶段，依次是入口指针在新版、回切旧壳、再切新版；阶段决定默认入口所属的 build。
const PHASES: [&str; 3] = ["new", "legacy", "new-again"];
const SURFACES: [&str; 2] = ["public", "admin"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SwitchDrillReceipt {
    format: String,
    drill_id: String,
    operator: String,
    reviewer: String,
    started_at: Option<Timestamp>,
    finished_at: Option<Timestamp>,
    artifact: DrillArtifact,
    surfaces: BTreeMap<String, DrillSurface>,
    responses: Vec<DrillResponse>,
    manual_signoff: ManualSignoff,
    anomalies: Vec<serde_json::Value>,
    conclusion: DrillConclusion,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillArtifact {
    tag: String,
    source_commit: String,
    archive_sha256: String,
    build_id: String,
}

/// 一端的两个 build：旧壳与新版 Solid，各带入口 HTML 的 SHA-256。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillSurface {
    legacy_build_id: String,
    legacy_entry_sha256: String,
    solid_build_id: String,
    solid_entry_sha256: String,
}

/// 一个响应的观测；只记事实，不记任何凭据或 URL 里的装备编号。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillResponse {
    phase: String,
    surface: String,
    /// `default-entry`（入口指针所指页面）、`independent-entry`（`/consignment` 独立入口）、
    /// `api-error`（未知 API 路由的 404，no-store）。
    kind: String,
    status: u16,
    x_auction_build: Option<String>,
    x_auction_build_independent: Option<String>,
    body_sha256: Option<String>,
    csp_sha256: String,
    etag: Option<String>,
    cache_control: Option<String>,
    vary: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualSignoff {
    wasm_started: ManualItem,
    legacy_interaction_state_changed: ManualItem,
    legacy_to_new_entry_read_ok: ManualItem,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualItem {
    result: String,
    evidence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DrillConclusion {
    result: String,
    summary: String,
}

/// 策略摘要必须逐字节对应期望的栈；放宽此比较（前缀、恒真）正是本演练要防的缺陷。
fn policy_matches(expected_csp: &str, observed_sha256: &str) -> bool {
    sha256_bytes(expected_csp.as_bytes()) == observed_sha256
}

impl SwitchDrillReceipt {
    /// 保护的不变量：旧 HTML 只配 Dioxus 策略、新版页面只配 Solid 策略，错配一律拒绝（不变量 6）。
    pub(super) fn validate(&self, requirements: &Requirements) -> anyhow::Result<DrillEvidence> {
        self.validate_identity(requirements)?;
        self.validate_artifact(requirements)?;
        self.validate_surfaces(requirements)?;
        self.validate_responses()?;
        self.validate_manual()?;
        self.validate_conclusion()?;
        Ok(DrillEvidence {
            drill_id: self.drill_id.clone(),
            tag: self.artifact.tag.clone(),
            build_id: self.artifact.build_id.clone(),
            responses: self.responses.len(),
        })
    }

    /// 格式、执行人与审核人分离，演练在最终实物创建之后开始并早于当前时间。
    fn validate_identity(&self, requirements: &Requirements) -> anyhow::Result<()> {
        if self.format != FORMAT {
            anyhow::bail!("页面切换演练回执格式不受支持：{}", self.format);
        }
        for (field, value) in [
            ("drill_id", &self.drill_id),
            ("operator", &self.operator),
            ("reviewer", &self.reviewer),
        ] {
            validate_text(field, value, 128)?;
        }
        if self.operator == self.reviewer {
            anyhow::bail!("页面切换演练回执必须由执行人以外的人审核");
        }
        let (Some(started), Some(finished)) = (self.started_at, self.finished_at) else {
            anyhow::bail!("页面切换演练回执缺少有效的开始与完成时间");
        };
        if started < requirements.artifact_created_at {
            anyhow::bail!("页面切换演练回执早于最终 tag 实物，演练的不是这份制品；请重做演练");
        }
        if finished < started || finished > requirements.now {
            anyhow::bail!("页面切换演练回执的起止时间不一致或在未来");
        }
        Ok(())
    }

    /// 演练所用实物必须就是部署回执登记的那一份。
    fn validate_artifact(&self, requirements: &Requirements) -> anyhow::Result<()> {
        let artifact = &self.artifact;
        if artifact.build_id != requirements.build_id {
            anyhow::bail!("页面切换演练回执的 build ID 与部署回执不一致");
        }
        if artifact.tag != requirements.tag
            || artifact.source_commit != requirements.source_commit
            || artifact.archive_sha256 != requirements.archive_sha256
            || !is_lower_hex(&artifact.source_commit, 40)
            || !is_lower_hex(&artifact.archive_sha256, 64)
        {
            anyhow::bail!("页面切换演练回执的 tag、提交或外层摘要与最终实物不一致");
        }
        Ok(())
    }

    /// 两端都要有；新版 build 等于实物的 build，旧壳是另一个 build，入口 HTML 摘要格式有效。
    fn validate_surfaces(&self, requirements: &Requirements) -> anyhow::Result<()> {
        if self.surfaces.len() != SURFACES.len()
            || SURFACES.iter().any(|surface| !self.surfaces.contains_key(*surface))
        {
            anyhow::bail!("页面切换演练回执必须恰好包含 public 与 admin 两端");
        }
        for (name, surface) in &self.surfaces {
            if surface.solid_build_id != requirements.build_id {
                anyhow::bail!("页面切换演练回执 {name} 端的新版 build 与实物不一致");
            }
            validate_text("legacy_build_id", &surface.legacy_build_id, 64)?;
            if surface.legacy_build_id == surface.solid_build_id
                || !is_lower_hex(&surface.legacy_entry_sha256, 64)
                || !is_lower_hex(&surface.solid_entry_sha256, 64)
            {
                anyhow::bail!("页面切换演练回执 {name} 端的 build 登记无效");
            }
        }
        Ok(())
    }

    /// 逐响应核对，再核每个（阶段、端、类别）都有完整、可相互印证的 200 与 304 观测。
    fn validate_responses(&self) -> anyhow::Result<()> {
        let mut etag_policy: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        let mut covered: BTreeSet<(&str, &str, &str, u16)> = BTreeSet::new();
        let mut full_etags: BTreeMap<(&str, &str, &str), &str> = BTreeMap::new();
        let mut revalidations: Vec<(&str, &str, &str, &str)> = Vec::new();
        for response in &self.responses {
            self.validate_response(response)?;
            let (phase, surface, kind) =
                (response.phase.as_str(), response.surface.as_str(), response.kind.as_str());
            covered.insert((phase, surface, kind, response.status));
            if kind == "api-error" {
                continue;
            }
            let etag = response.etag.as_deref().unwrap_or_default();
            // 一个验证器只能标识一种策略：同端同验证器配不同策略，说明 304 可能复用错版本。
            if let Some(previous) = etag_policy.insert((surface, etag), &response.csp_sha256)
                && previous != response.csp_sha256
            {
                anyhow::bail!("页面切换演练回执中同一 ETag 对应了两种脚本策略（{surface}）");
            }
            if response.status == 200 {
                full_etags.insert((phase, surface, kind), etag);
            } else {
                revalidations.push((phase, surface, kind, etag));
            }
        }
        // 304 回显的验证器必须是同一（阶段、端、类别）下 200 的验证器，与记录顺序无关。
        for (phase, surface, kind, etag) in revalidations {
            if full_etags.get(&(phase, surface, kind)) != Some(&etag) {
                anyhow::bail!("页面切换演练回执的 304 验证器与同入口的 200 不一致");
            }
        }
        for phase in PHASES {
            for surface in SURFACES {
                for (kind, statuses) in [
                    ("default-entry", &[200_u16, 304][..]),
                    ("independent-entry", &[200, 304][..]),
                    ("api-error", &[404][..]),
                ] {
                    for status in statuses {
                        if !covered.contains(&(phase, surface, kind, *status)) {
                            anyhow::bail!(
                                "页面切换演练回执缺少观测：阶段 {phase}、{surface} 端、{kind}、状态 {status}"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_response(&self, response: &DrillResponse) -> anyhow::Result<()> {
        let Some(surface) = self.surfaces.get(&response.surface) else {
            anyhow::bail!("页面切换演练回执含未知的端：{}", response.surface);
        };
        // 入口指针在回切阶段指向旧壳；其余阶段指向新版。独立入口与 API 错误页永远是 Solid。
        let legacy_pointer = match response.phase.as_str() {
            "legacy" => true,
            "new" | "new-again" => false,
            other => anyhow::bail!("页面切换演练回执含未知的阶段：{other}"),
        };
        let (pointer_build, pointer_csp) = if legacy_pointer {
            (&surface.legacy_build_id, DIOXUS_CSP)
        } else {
            (&surface.solid_build_id, SOLID_CSP)
        };
        let (expected_csp, expected_html) = match response.kind.as_str() {
            "default-entry" if legacy_pointer => (pointer_csp, &surface.legacy_entry_sha256),
            "default-entry" | "independent-entry" => (SOLID_CSP, &surface.solid_entry_sha256),
            "api-error" => (SOLID_CSP, &surface.solid_entry_sha256),
            other => anyhow::bail!("页面切换演练回执含未知的响应类别：{other}"),
        };
        if !policy_matches(expected_csp, &response.csp_sha256) {
            anyhow::bail!(
                "页面切换演练回执的脚本策略与所属 build 不符：阶段 {}、{} 端、{}",
                response.phase,
                response.surface,
                response.kind
            );
        }
        if response.kind == "api-error" {
            // no-store 响应同时携带指针构建号与独立构建号。
            if response.status != 404
                || response.body_sha256.is_some()
                || response.cache_control.as_deref() != Some("no-store")
                || response.x_auction_build.as_deref() != Some(pointer_build.as_str())
                || response.x_auction_build_independent.as_deref()
                    != Some(surface.solid_build_id.as_str())
            {
                anyhow::bail!("页面切换演练回执的 API 错误响应与构建号不符");
            }
            return Ok(());
        }
        // 入口 HTML 是 no-cache 的可重放响应，服务端不发构建号头；build 由入口 HTML 摘要识别。
        // 入口响应带了构建号头说明服务端行为漂移，失败关闭。
        if response.x_auction_build.is_some() || response.x_auction_build_independent.is_some() {
            anyhow::bail!("页面切换演练回执的入口响应不应带构建号头");
        }
        if !response.etag.as_deref().is_some_and(|etag| !etag.is_empty()) {
            anyhow::bail!("页面切换演练回执的入口响应缺少 ETag");
        }
        match response.status {
            200 => {
                if response.body_sha256.as_deref() != Some(expected_html.as_str())
                    || response.cache_control.as_deref() != Some("no-cache")
                    || response.vary.as_deref() != Some("Accept-Encoding")
                {
                    anyhow::bail!(
                        "页面切换演练回执的入口 HTML 摘要或缓存头与所属 build 不符：阶段 {}、{} 端、{}",
                        response.phase,
                        response.surface,
                        response.kind
                    );
                }
            }
            304 => {
                if response.body_sha256.is_some() {
                    anyhow::bail!("页面切换演练回执的 304 不应带正文摘要");
                }
            }
            other => anyhow::bail!("页面切换演练回执的入口响应状态无效：{other}"),
        }
        Ok(())
    }

    /// 浏览器步骤必须全部由人工签收通过并附证据名；缺任一项即拒绝。
    fn validate_manual(&self) -> anyhow::Result<()> {
        for (name, item) in [
            ("wasm_started", &self.manual_signoff.wasm_started),
            (
                "legacy_interaction_state_changed",
                &self.manual_signoff.legacy_interaction_state_changed,
            ),
            ("legacy_to_new_entry_read_ok", &self.manual_signoff.legacy_to_new_entry_read_ok),
        ] {
            if item.result != "passed" || item.evidence.trim().is_empty() {
                anyhow::bail!("页面切换演练回执的人工签收项 {name} 未通过或缺少证据");
            }
        }
        Ok(())
    }

    fn validate_conclusion(&self) -> anyhow::Result<()> {
        if !self.anomalies.is_empty() {
            anyhow::bail!("页面切换演练回执记录了未解释的异常，不能作为签收依据");
        }
        if self.conclusion.result != "passed" {
            anyhow::bail!("页面切换演练回执的结论不是 passed");
        }
        validate_text("conclusion.summary", &self.conclusion.summary, 4096)
    }
}

fn validate_text(field: &str, value: &str, max: usize) -> anyhow::Result<()> {
    if !text::single_line(value, max) {
        anyhow::bail!("页面切换演练回执的 {field} 无效");
    }
    Ok(())
}
