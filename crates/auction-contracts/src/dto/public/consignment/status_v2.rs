//! v2 寄售能力：两世界并行状态、季界与未来可放开的购买能力。

use auction_time::Timestamp;

use crate::dto::public::consignment::scalar::ListingFeeBps;
use crate::dto::public::consignment::states::ListingMode;
use crate::dto::public::consignment::states::ListingWorld;
use crate::dto::public::consignment::wire::PRICE_MAX;
use crate::dto::public::consignment::wire::json_object;
use crate::dto::public::consignment::wire::optional_timestamp;
use crate::dto::public::consignment::wire_v2::string_enum;

string_enum! {
    /// 冻结季历计算出的季态闭集。
    pub enum SeasonStateV2 {
        /// 该世界没有赛季。
        NotSeasonal => "not_seasonal",
        /// 正常开放。
        Open => "open",
        /// 已进入停止新预约与购买的预停窗口。
        Preclose => "preclose",
        /// 季界已关闭。
        Closed => "closed",
        /// 当前日历无法判定安全季界。
        Unknown => "unknown",
    }
}

json_object! {
    ()
    /// 单个世界的真实收件与购买能力。购买能力在第 4 期恒由服务端输出 false，
    /// 但本契约保留布尔类型，供后续期在同一 v2 形状内放开。
    ///
    /// 第 4 期合同固定季界形状：`not_seasonal` 与 `unknown` 时两个时刻必须为 null，
    /// `unknown` 另要求两种能力均为 false；`open/preclose/closed` 时两个时刻必须非 null。
    /// 产生方按此严格口径输出。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WorldCapabilityV2 {
        pub mail_ready: bool,
        pub purchase_ready: bool,
        pub season_state: SeasonStateV2,
        #[serde(deserialize_with = "optional_timestamp")]
        pub cutoff_at: Option<Timestamp>,
        #[serde(deserialize_with = "optional_timestamp")]
        pub season_end_at: Option<Timestamp>,
    } => validate
}

impl WorldCapabilityV2 {
    fn validate(&self) -> Result<(), &'static str> {
        match self.season_state {
            SeasonStateV2::NotSeasonal | SeasonStateV2::Unknown => {
                if self.cutoff_at.is_some() || self.season_end_at.is_some() {
                    return Err("无赛季或季历未知时两个时刻必须为 null");
                }
            }
            SeasonStateV2::Open | SeasonStateV2::Preclose | SeasonStateV2::Closed => {
                if self.cutoff_at.is_none() || self.season_end_at.is_none() {
                    return Err("已知季态必须同时给出预停与季末时刻");
                }
            }
        }
        if self.season_state == SeasonStateV2::Unknown && (self.mail_ready || self.purchase_ready) {
            return Err("季历未知时不得宣告收件或购买能力");
        }
        Ok(())
    }
}

json_object! {
    ()
    /// 键恰为 `main` 与 `isekai` 的两世界能力对象。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ConsignmentWorldsV2 {
        pub main: WorldCapabilityV2,
        pub isekai: WorldCapabilityV2,
    }
}

json_object! {
    ()
    /// 匿名可读的 v2 寄售能力。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ConsignmentStatusV2 {
        pub enabled: bool,
        pub registration_ready: bool,
        pub registration_modes: Vec<ListingMode>,
        pub registration_worlds: Vec<ListingWorld>,
        pub fee_bps: ListingFeeBps,
        pub min_price: i64,
        pub max_price: i64,
        pub accepts_mail: bool,
        pub purchasable: bool,
        pub worlds: ConsignmentWorldsV2,
    } => validate
}

impl ConsignmentStatusV2 {
    fn validate(&self) -> Result<(), &'static str> {
        if self.min_price != 1 || self.max_price != PRICE_MAX {
            return Err("寄售登记价格上下限须为协议常量");
        }
        if self.registration_ready && !self.enabled {
            return Err("寄售关闭时不可登记");
        }
        if !self.registration_ready
            && !(self.registration_modes.is_empty() && self.registration_worlds.is_empty())
        {
            return Err("登记未就绪时不得提供可登记模式和世界");
        }
        if self.registration_ready
            && (self.registration_modes.is_empty() || self.registration_worlds.is_empty())
        {
            return Err("登记就绪时必须给出至少一个模式与世界");
        }
        if self.worlds.main.season_state != SeasonStateV2::NotSeasonal {
            return Err("main 没有赛季，季态必须为 not_seasonal");
        }
        if self.worlds.isekai.season_state == SeasonStateV2::NotSeasonal {
            return Err("isekai 必须有赛季，季态不得为 not_seasonal");
        }
        // D9 §6.1：汇总取各世界 OR。第 4 期的生产者把购买两字段都输出 false，
        // 但契约不把它们写死，后续期可在同一形状内放开。
        if self.accepts_mail != (self.worlds.main.mail_ready || self.worlds.isekai.mail_ready) {
            return Err("汇总收件能力必须等于两世界 OR");
        }
        if self.purchasable
            != (self.worlds.main.purchase_ready || self.worlds.isekai.purchase_ready)
        {
            return Err("汇总购买能力必须等于两世界 OR");
        }
        Ok(())
    }
}
