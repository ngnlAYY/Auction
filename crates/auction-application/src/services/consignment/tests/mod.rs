//! 用 2P 替身检查寄售用例的编排和失败边界。

use std::future::Future;
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingPrice, ListingVersion};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::public::{AdminSessionIdentity, PublicSessionIdentity};

mod maintenance;
mod manage;
mod register;
mod resolve;

const LISTING_ID: Uuid = Uuid::from_u128(42);
const WORK_ID: Uuid = Uuid::from_u128(43);
const SELLER_ID: Uuid = Uuid::from_u128(44);
const INVALID_CLIENT_COMMAND_IDS: [Uuid; 4] = [
    uuid::uuid!("00000000-0000-1000-8000-000000000045"),
    uuid::uuid!("00000000-0000-5000-8000-000000000045"),
    uuid::uuid!("00000000-0000-7000-8000-000000000045"),
    uuid::uuid!("00000000-0000-4000-0000-000000000045"),
];

const FAKE_KEY: &str = "face00face";
const FAKE_URL: &str = "https://example.invalid/equip/42/face00face";

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..4 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("进程内替身未在预期轮数内完成")
}

fn command_id() -> CommandId {
    CommandId::from_uuid(uuid::uuid!("00000000-0000-4000-8000-000000000045"))
}

fn time(second: i64) -> Timestamp {
    Timestamp::from_timestamp(second, 0).unwrap()
}

fn seller() -> PublicSessionIdentity {
    PublicSessionIdentity {
        user_id: SELLER_ID,
        username: "测试卖家".to_owned(),
        expires_at: None,
        absolute_expires_at: None,
    }
}

fn admin() -> AdminSessionIdentity {
    AdminSessionIdentity {
        actor: "测试管理员".to_owned(),
        expires_at: time(10),
        absolute_expires_at: time(20),
    }
}

fn identity() -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new("isekai", "42").unwrap()
}

fn price(credits: i64) -> ListingPrice {
    ListingPrice::new(credits).unwrap()
}

fn version(value: u64) -> ListingVersion {
    ListingVersion::new(value).unwrap()
}
