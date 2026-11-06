//! shadow_run 上下文公开 API。

pub use crate::ports::shadow_run::{
    ShadowOutcome, ShadowOwner, ShadowRecorder, ShadowSample, noop_shadow_recorder,
    record_comparison, record_sample, record_uncomparable,
};
