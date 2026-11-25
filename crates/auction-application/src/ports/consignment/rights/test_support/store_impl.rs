//! `FakeRightsStore` 对 `RightsStore` 端口的实现：全部委托给同步的 `do_*` 方法。

use auction_domain::CommandId;
use auction_domain::consignment::rights::{RightView, SessionDigest};

use super::FakeRightsStore;
use crate::ports::consignment::{
    BrowserBinding, ChallengeIssued, ChallengeRightCommand, ClaimOutcome, ClaimRightCommand,
    LocateRightCommand, RightReceipt, RightVerified, RightsFuture, RightsStore, VerifyRightCommand,
};

impl RightsStore for FakeRightsStore {
    fn locate(&self, command: LocateRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        Box::pin(std::future::ready(self.do_locate(command)))
    }

    fn challenge(&self, command: ChallengeRightCommand) -> RightsFuture<'_, ChallengeIssued> {
        Box::pin(std::future::ready(self.do_challenge(command)))
    }

    fn verify(&self, command: VerifyRightCommand) -> RightsFuture<'_, RightVerified> {
        Box::pin(std::future::ready(self.do_verify(command)))
    }

    fn current(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
    ) -> RightsFuture<'_, Option<RightView>> {
        Box::pin(std::future::ready(Ok(self.do_current(session, browser))))
    }

    fn claim(&self, command: ClaimRightCommand) -> RightsFuture<'_, ClaimOutcome> {
        Box::pin(std::future::ready(self.do_claim(command)))
    }

    fn command_receipt(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
        command_id: CommandId,
    ) -> RightsFuture<'_, Option<RightReceipt>> {
        Box::pin(std::future::ready(Ok(self.do_receipt(session, browser, command_id))))
    }
}
