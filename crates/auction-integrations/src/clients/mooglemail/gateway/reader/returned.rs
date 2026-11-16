//! 结算退件只读核验：按 MID 读取 Bot 收件箱中的一封邮件详情，只投影发件人、主题、正文事实与
//! 资产。不领取、不退回；正文只在本函数内解析为原收件人、摘要与派发引用后即丢弃，不进入日志。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::settlement::{
    SettlementReturnMailObservation, SettlementReturnRecheckOutcome, parse_returned_settlement_body,
};
use chrono::{DateTime, Utc};

use super::MoogleMailGateway;
use super::error::fetch_budget_exhausted;
use crate::clients::mooglemail::error::{MoogleMailError, MoogleMailErrorCategory};
use crate::clients::mooglemail::gateway::{configuration, operation_error};
use crate::clients::mooglemail::protocol::{MoogleMailOperation, MoogleMailWorld, valid_opaque_id};
use crate::safe_diagnostics;

impl MoogleMailGateway {
    /// 读取收件箱中一封退件的只读事实；只有完整的“邮件不存在”提示才返回未找到，登录页、结构
    /// 漂移与超时都按错误返回，由调用方保持核验待办。
    pub async fn read_returned_mail(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<SettlementReturnRecheckOutcome, MoogleMailError> {
        if !valid_opaque_id(external_mail_id) || observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Detail,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session =
                self.credentials.begin_read(world, MoogleMailOperation::Detail).await?;
            let target = self.client.detail(world, external_mail_id)?;
            let document = match self.client.get(&target, session.cookie()).await {
                Ok(document) => document,
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let state = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.parse_action_state_or_missing(&document, world),
                )
                .await?;
            session.check_current().await?;
            Ok(match state {
                None => SettlementReturnRecheckOutcome::NotFound { observed_at },
                Some(state) => {
                    let mut attachment_ids = state.equipment_ids.clone();
                    attachment_ids.sort_unstable();
                    SettlementReturnRecheckOutcome::Observed(SettlementReturnMailObservation {
                        sender: state.sender.clone(),
                        subject: state.subject.clone(),
                        body: parse_returned_settlement_body(&state.body),
                        credits: state.credits,
                        cod: state.cod,
                        attachment_ids,
                        observed_at,
                    })
                }
            })
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_returned_mail_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }
}
