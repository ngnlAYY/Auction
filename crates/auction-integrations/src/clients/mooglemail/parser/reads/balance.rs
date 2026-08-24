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
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let source =
            str::from_utf8(document).map_err(|_| protocol(MoogleMailOperation::Balance, world))?;
        if source.trim().is_empty() {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let document = Html::parse_document(source);
        let networth_selector = selector("#networth", MoogleMailOperation::Balance, world)?;
        let networth = document.select(&networth_selector).collect::<Vec<_>>();
        if networth.len() != 1 {
            return Err(protocol(MoogleMailOperation::Balance, world));
        }
        let visible_credits =
            parse_networth_credits(&normalize_text(networth[0].text().collect::<String>()));
        let script = script_text(&document, world, MoogleMailOperation::Balance)?;
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
