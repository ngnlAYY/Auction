//! sli 上下文公开 API。

pub use crate::ports::sli::{
    SLI_AWAITING_INDEX, SLI_CANDIDATE_LIMIT, SLI_SLOW_PROXY_FLOW, SLI_WINDOW_SECONDS,
    SliBidCapacity, SliBidKind, SliDeadlineState, SliFlow, SliFlowWindow, SliOutbidChannel,
    SliOutbidMargin, SliPending, SliSnapshot, SliSnapshotError, SliSnapshotReader, SliWorld,
};
