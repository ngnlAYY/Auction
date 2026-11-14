//! compose 草稿的只读投影与受控 discard；两者共用窄互斥的 compose 读取。
//!
//! 观测本身不提升为投递成功；discard 只丢弃调用方已精确匹配到本地已知投递的草稿。

use auction_application::core::diagnostics::DiagnosticContext;

use super::super::{MoogleMailSender, evidence::write_unknown};
use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

/// compose 页当前状态的只读投影。
pub struct MoogleMailComposeSnapshot {
    pub recipient: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: Option<i64>,
    pub attachment_count: usize,
    /// 字典序排列的精确装备集合，不以附件计数代替资产身份。
    pub equipment_ids: Vec<String>,
    pub clean: bool,
}

impl MoogleMailComposeSnapshot {
    pub const fn is_clean(&self) -> bool {
        self.clean
    }
}

/// 受控 discard 的结果；已干净时不做任何远端写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailDiscardOutcome {
    AlreadyClean,
    Discarded,
}

impl MoogleMailSender {
    /// 只读读取 compose 页，返回草稿字段；解析失败按协议漂移处理。
    pub async fn inspect_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailComposeSnapshot, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        let clean = state.is_clean();
        let mut fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().to_vec();
        equipment_ids.sort_unstable();
        Ok(MoogleMailComposeSnapshot {
            recipient: std::mem::take(&mut fields.recipient),
            subject: std::mem::take(&mut fields.subject),
            body: std::mem::take(&mut fields.body),
            credits: state.credits(),
            cod: state.cod(),
            attachment_count: state.attachment_count(),
            equipment_ids,
            clean,
        })
    }

    /// 丢弃 compose 草稿并在 POST 后重新读取证明页面已干净。
    /// 只允许对已登记一次性尝试且精确匹配已知投递的草稿调用；失败只读核对，不自动重发 discard。
    pub async fn discard_compose(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        expected: &auction_application::mail::MoogleMailboxComposeState,
    ) -> Result<MoogleMailDiscardOutcome, MoogleMailError> {
        let compose = self.client.compose(world)?;
        let document = self.client.get(&compose, cookie).await?;
        let state = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await?;
        if state.is_clean() {
            return Ok(MoogleMailDiscardOutcome::AlreadyClean);
        }
        let fields = state.message_fields();
        let mut equipment_ids = state.equipment_ids().iter().collect::<Vec<_>>();
        equipment_ids.sort_unstable();
        // 原快照与本次 POST 之间重新取得了 session；持锁重读后逐字段一致才有权丢弃。
        if expected.clean
            || fields.recipient != expected.recipient
            || fields.subject != expected.subject
            || fields.body != expected.body
            || state.credits() != expected.credits
            || state.cod() != expected.cod
            || state.attachment_count() != expected.attachment_count
            || !equipment_ids.into_iter().eq(expected.equipment_ids.iter())
        {
            return Err(protocol_drift(world));
        }
        let form = MoogleMailForm::discard(state.token(), &fields).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "build_discard_form",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            protocol_drift(world)
        })?;
        self.client.post(&compose, cookie, form).await?;
        let document = self.client.get(&compose, cookie).await.map_err(discard_unknown)?;
        let after = self
            .client
            .diagnose_parse(&compose, &document, self.parser.parse_compose_state(&document, world))
            .await
            .map_err(discard_unknown)?;
        if after.is_clean() {
            Ok(MoogleMailDiscardOutcome::Discarded)
        } else {
            Err(write_unknown(world, MoogleMailOperation::Discard))
        }
    }
}

fn protocol_drift(world: MoogleMailWorld) -> MoogleMailError {
    MoogleMailError::protocol_drift(MoogleMailOperation::Compose, world)
}

/// POST 后的读取/解析失败仍属未知外写；保留已有加密诊断引用，不降级成可重试读错误。
fn discard_unknown(error: MoogleMailError) -> MoogleMailError {
    let category = if error.category == MoogleMailErrorCategory::SessionExpired {
        MoogleMailErrorCategory::SessionExpired
    } else {
        MoogleMailErrorCategory::WriteUnknown
    };
    let mut unknown =
        MoogleMailError::after_write(MoogleMailOperation::Discard, error.world, category);
    unknown.evidence_id = error.evidence_id;
    unknown.evidence_status = error.evidence_status;
    unknown.evidence_failure = error.evidence_failure;
    unknown
}
