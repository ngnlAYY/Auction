//! 从看板的权威场次快照取得冻结命令所需的世界与场次。

use dioxus::prelude::*;

use super::request::CommandAuction;
use crate::surface::{CurrentAuctionResource, PublicBoardQuery, PublicDisplayedAuction};

/// 同一目录的各行共用场次，避免每行订阅整个会话无关的看板快照。
#[derive(Clone, Copy)]
pub(in crate::surface) struct CommandAuctionContext(pub Memo<Option<CommandAuction>>);

pub(in crate::surface) fn use_command_auction_provider() {
    let query = try_use_context::<Signal<PublicBoardQuery>>();
    let selected = try_use_context::<Signal<String>>();
    let displayed = try_use_context::<PublicDisplayedAuction>();
    let current = try_use_context::<CurrentAuctionResource>();
    let location = use_memo(move || {
        let requested = query.and_then(|query| query.read().auction_id.clone());
        if let Some(requested) = requested {
            return displayed?.0.read().as_ref().filter(|auction| auction.id == requested).map(
                |auction| CommandAuction {
                    auction_id: auction.id.clone(),
                    world: auction.world.clone(),
                },
            );
        }
        let selected = selected?;
        let current = current?;
        let current = current.read();
        let (loaded_world, result) = current.as_ref()?;
        let world = selected.read();
        let auction = result.as_ref().ok()?.as_ref()?;
        (loaded_world == &*world && auction.world == *world).then(|| CommandAuction {
            auction_id: auction.id.clone(),
            world: auction.world.clone(),
        })
    });
    use_context_provider(|| CommandAuctionContext(location));
}
