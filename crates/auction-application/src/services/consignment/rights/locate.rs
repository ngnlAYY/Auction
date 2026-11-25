//! 找回：按游戏身份与世界请求定位符指引。

use std::net::IpAddr;

use auction_domain::SettlementWorld;
use auction_domain::consignment::settlement_world_code;
use uuid::Uuid;

use super::{RightOperation, RightsService, parse_command_id, rights_encoding};
use crate::ports::consignment::{
    BrowserBinding, ChallengeIssued, GameIdentity, LocateRightCommand, RightsStoreError,
};
use crate::services::consignment::deposit::encoding::CommandEncoding;

/// 找回输入。
#[derive(Debug, Clone)]
pub struct LocateRightInput {
    /// 浏览器 UUIDv4。
    pub command_id: Uuid,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 可信客户端 IP。
    pub client_ip: IpAddr,
    /// 游戏身份原文。
    pub game_identity: String,
    /// 世界。
    pub world: SettlementWorld,
}

impl RightsService {
    /// 找回；身份形状无效为 `InvalidRequest`，其余与目标真实性无关。
    pub async fn locate(
        &self,
        input: LocateRightInput,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let command_id = parse_command_id(input.command_id)?;
        let game_identity =
            GameIdentity::parse(&input.game_identity).ok_or(RightsStoreError::InvalidRequest)?;
        let request_hash =
            canonical_locate(input.browser, &game_identity, input.world).request_hash();
        self.store()
            .locate(LocateRightCommand {
                command_id,
                browser: input.browser,
                client_ip: input.client_ip,
                game_identity,
                world: input.world,
                request_hash,
            })
            .await
    }
}

/// 找回的 v2 规范编码：actor 与目标都是原浏览器，输入为身份原文与世界码。
pub(super) fn canonical_locate(
    browser: BrowserBinding,
    game_identity: &GameIdentity,
    world: SettlementWorld,
) -> CommandEncoding {
    let mut encoding = rights_encoding(RightOperation::Locate, browser);
    encoding.text(game_identity.as_str());
    encoding.text(settlement_world_code(world));
    encoding
}
