//! Public 寄售 v2 购买、订单读取、公开目录、卖家重列与损失回执、受限权利的装配。
//!
//! 数据库可用即装配读写端口：寄售软关闭只截止新承诺（购买、重列由处理器按开关拒绝新命令），
//! 本人订单与损失回执读取、已接受购买的同 ID 重放与持有人续办旧权利照常（D9 §7，不变量 1）。

use std::sync::Arc;

use anyhow::Context;
use auction_application::consignment::{
    ManualRelistService, PurchaseCommandService, RightChallengeCrypto, RightsService,
};
use auction_config::{AppConfig, ConsignmentActivation};
use auction_http::PublicApiState;
use auction_integrations::{ConsignmentRightChallengeCrypto, Keyring};
use auction_storage::{
    ActiveBotFundingPreflight, DatabaseStore, DepositRepository, DepositSettings, PgRightsStore,
    PurchaseRepository,
};

use super::first_dispatch::FrozenFirstDispatchContent;

/// 把购买、订单读取、公开 v2 目录、卖家重列与损失回执、受限权利注入 Public 状态。
///
/// 购买：资金 UID 每次从数据库权威来源读取（main 收件就绪会话所绑定的当前 active `mail_cookie`
/// 凭据的认证 Bot UID），不读配置、不硬编码；缺失时该次购买 503、不写回执。首轮邮件内容按 5C
/// 冻结模板渲染；Sale 恢复策略取启动时冻结的配置，成交时写进合同（不变量 10）。
///
/// 不变量 7：挑战码的封装与 MAC 只用凭据域密钥环；没有密钥环时不装配密码端口，存储对找回、发码与
/// 核验失败关闭为 503。密钥环由装配根在同一次启动里只解析一次：寄售激活时缺失或无效已在预约码
/// 加密处拒绝启动；寄售未激活时无效配置只告警并按没有密钥环传入，不给既有拍卖增加启动失败条件，
/// 代价是关闭期间权利续办不可用，直到修好配置并重启。
pub(super) fn configure(
    state: PublicApiState,
    config: &AppConfig,
    activation: ConsignmentActivation,
    store: &Arc<DatabaseStore>,
    credential_keyring: Option<Arc<Keyring>>,
) -> anyhow::Result<PublicApiState> {
    let enabled = activation == ConsignmentActivation::Enabled;
    let consignment = &config.consignment;
    let crypto = credential_keyring.map(|keyring| {
        let crypto: Arc<dyn RightChallengeCrypto> =
            Arc::new(ConsignmentRightChallengeCrypto::new(keyring));
        crypto
    });
    let rights =
        RightsService::new(Arc::new(PgRightsStore::new((**store).clone(), crypto.clone())), crypto);
    let purchases = Arc::new(PurchaseRepository::new(
        (**store).clone(),
        Arc::new(ActiveBotFundingPreflight::new((**store).clone())),
        Arc::new(FrozenFirstDispatchContent),
        consignment.sale_policy(),
    ));
    let purchase = PurchaseCommandService::new(purchases.clone(), purchases);
    // 重列只用额度上限，不需要预约码密码；设置与收件端口同源于同一份配置。这里的构造依赖
    // `state::configure_deposit` 已对同一份配置无条件校验过（统筹方裁定），因而不会为寄售未激活的
    // 进程引入新的启动失败条件。
    let settings = DepositSettings::with_standard_price_bounds(
        consignment.fee_bps(),
        consignment.reservation_ttl_hours(),
        consignment.max_open_per_user(),
    )
    .context("寄售收件设置无效")?;
    let relist = ManualRelistService::new(Arc::new(DepositRepository::without_reservation_crypto(
        (**store).clone(),
        settings,
    )));
    Ok(state
        .with_purchase(enabled, Some(Arc::new(purchase)), Some(store.clone()))
        .with_orders(Some(store.clone()))
        .with_recovery(enabled, Some(Arc::new(relist)), Some(store.clone()))
        .with_command_receipts(Some(store.clone()))
        .with_rights(Some(Arc::new(rights))))
}
