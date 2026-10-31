//! MoogleMail Item Shop 余额解析。

use super::super::*;

impl MoogleMailParser {
    /// 严格解析 Item Shop 余额，并要求可见余额与脚本余额唯一且一致。
    pub fn parse_balance(
        &self,
        document: &[u8],
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        let document =
            super::identity::trusted_document(document, world, MoogleMailOperation::Balance)?;
        self.parse_balance_document(&document, world, observed_at)
    }

    /// 联合证据复用已完成世界证明的 DOM，余额本身仍须通过完整结构校验。
    pub(crate) fn parse_balance_document(
        &self,
        document: &Html,
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let networth_selector = selector("#networth", MoogleMailOperation::Balance, world)?;
        let networth = unique_element(
            document.select(&networth_selector),
            MoogleMailOperation::Balance,
            world,
        )?;
        let visible_credits =
            parse_networth_credits(&normalize_text(networth.text().collect::<String>()));
        let script = script_text(document, world, MoogleMailOperation::Balance)?;
        let executable = executable_javascript(&script)
            .ok_or_else(|| protocol(MoogleMailOperation::Balance, world))?;
        let script_credits = required_script_value(&executable, "current_credits");
        if visible_credits.is_none()
            || script_credits.is_none()
            || visible_credits != script_credits
        {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let Some(credits) = visible_credits else {
            return Err(protocol(MoogleMailOperation::Balance, world));
        };
        let snapshot = MoogleMailBalanceSnapshot { world, credits, observed_at };
        if !snapshot.validate() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        Ok(snapshot)
    }
}
