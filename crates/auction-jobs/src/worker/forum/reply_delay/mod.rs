//! 回帖导入延迟的有界、只读观测估计。
//!
//! 终点是每个回帖第一条新命令账本提交得到确认，不带任何业务裁决。`poll_window` 依赖
//! 完整主题扫描与稳定可见性：上次完整扫描开始为 A，本次完整扫描完成为 B，导入确认为 I，
//! 延迟区间为 `[I-B, I-A]`；中点只是统计估计。首次扫描、覆盖不完整、重启或状态淘汰时
//! 不填零，只记录覆盖结果。

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use auction_application::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumReplyPhase, ForumWorld, ReplyDelayMethod,
    ReplyDelayPhase, ReplyDelaySampleOutcome,
};
use auction_domain::{AuctionId, SettlementWorld};

const MAX_TOPICS: usize = 512;
const MAX_POST_IDS_PER_SCAN: usize = 4096;
const MAX_CANDIDATES_PER_SCAN: usize = 256;
const MAX_EVICTED_TOPICS: usize = 256;

struct ScanWindow {
    started: Instant,
    post_ids: HashSet<String>,
    truncated: bool,
}

struct ActiveScan {
    token: u64,
    auction_id: AuctionId,
    world: ForumWorld,
    phase: ReplyDelayPhase,
    started: Instant,
    completed: Instant,
    post_ids: HashSet<String>,
    truncated: bool,
    candidates: Vec<(String, Instant)>,
    candidate_ids: HashSet<String>,
    overflow_ids: HashSet<String>,
}

struct TopicBaseline {
    scan: ScanWindow,
    /// 已确认导入过的帖子；跨扫描保留，避免同一帖的后续命令替代首次确认终点。
    seen_imports: HashSet<String>,
    touched: u64,
}

struct EstimatorState {
    active: Option<ActiveScan>,
    topics: HashMap<AuctionId, TopicBaseline>,
    evicted: HashSet<AuctionId>,
    eviction_order: VecDeque<AuctionId>,
    next_token: u64,
    next_touch: u64,
}

/// 单进程内有界的回帖延迟估计器；不参与扫描、命令、租约或重试。
pub(super) struct ReplyDelayEstimator {
    observer: Arc<dyn ExternalMetricsObserver>,
    state: Mutex<EstimatorState>,
}

impl ReplyDelayEstimator {
    pub(super) fn new(observer: Arc<dyn ExternalMetricsObserver>) -> Self {
        Self {
            observer,
            state: Mutex::new(EstimatorState {
                active: None,
                topics: HashMap::new(),
                evicted: HashSet::new(),
                eviction_order: VecDeque::new(),
                next_token: 1,
                next_touch: 1,
            }),
        }
    }

    /// 在一次完整主题读取与固定楼层校验后开启观测窗口。若上一窗口未收口，按覆盖不完整处理。
    pub(super) fn begin_scan(
        &self,
        auction_id: AuctionId,
        world: SettlementWorld,
        phase: ForumReplyPhase,
        started: Instant,
        completed: Instant,
        post_ids: impl IntoIterator<Item = String>,
    ) -> ReplyDelayScanGuard<'_> {
        let mut state = lock_state(&self.state);
        if let Some(active) = state.active.take() {
            flush_scan(&self.observer, &mut state, active, false);
        }
        let token = state.next_token;
        state.next_token = state.next_token.wrapping_add(1).max(1);
        let mut ids = HashSet::new();
        let mut truncated = false;
        for post_id in post_ids {
            if ids.len() >= MAX_POST_IDS_PER_SCAN {
                truncated = true;
                break;
            }
            ids.insert(post_id);
        }
        state.active = Some(ActiveScan {
            token,
            auction_id,
            world: forum_world(world),
            phase: reply_delay_phase(phase),
            started,
            completed,
            post_ids: ids,
            truncated,
            candidates: Vec::new(),
            candidate_ids: HashSet::new(),
            overflow_ids: HashSet::new(),
        });
        ReplyDelayScanGuard { estimator: self, token, finished: false }
    }

    /// 在命令账本首次插入返回后调用；同一扫描窗口内同一帖只保留第一次确认。
    pub(super) fn observe_import(
        &self,
        auction_id: AuctionId,
        world: SettlementWorld,
        phase: ForumReplyPhase,
        post_id: &str,
        confirmed_at: Instant,
    ) {
        let mut state = lock_state(&self.state);
        // 同一帖只认第一次导入确认；先前不完整扫描或已采样帖子的后续命令不再覆盖终点。
        if state.topics.get(&auction_id).is_some_and(|topic| topic.seen_imports.contains(post_id)) {
            return;
        }
        let Some(active) = state.active.as_mut() else {
            record_sample(
                &self.observer,
                forum_world(world),
                reply_delay_phase(phase),
                ReplyDelaySampleOutcome::MissingBaseline,
            );
            return;
        };
        if active.auction_id != auction_id {
            record_sample(
                &self.observer,
                active.world,
                active.phase,
                ReplyDelaySampleOutcome::CoverageUnknown,
            );
            return;
        }
        if !active.post_ids.contains(post_id) {
            let outcome = if active.truncated {
                ReplyDelaySampleOutcome::StateEvicted
            } else {
                ReplyDelaySampleOutcome::CoverageUnknown
            };
            record_sample(&self.observer, active.world, active.phase, outcome);
            return;
        }
        if active.candidate_ids.contains(post_id) {
            return;
        }
        if active.candidate_ids.len() >= MAX_CANDIDATES_PER_SCAN {
            if active.overflow_ids.insert(post_id.to_owned()) {
                record_sample(
                    &self.observer,
                    active.world,
                    active.phase,
                    ReplyDelaySampleOutcome::StateEvicted,
                );
            }
            return;
        }
        active.candidate_ids.insert(post_id.to_owned());
        active.candidates.push((post_id.to_owned(), confirmed_at));
    }

    fn finish_scan(&self, token: u64, complete: bool) {
        let mut state = lock_state(&self.state);
        let Some(active) = state.active.take() else { return };
        if active.token != token {
            state.active = Some(active);
            return;
        }
        flush_scan(&self.observer, &mut state, active, complete);
    }
}

/// 扫描结束或取消时收口；取消不是零延迟样本。
pub(super) struct ReplyDelayScanGuard<'a> {
    estimator: &'a ReplyDelayEstimator,
    token: u64,
    finished: bool,
}

impl ReplyDelayScanGuard<'_> {
    pub(super) fn finish(mut self, complete: bool) {
        self.finished = true;
        self.estimator.finish_scan(self.token, complete);
    }
}

impl Drop for ReplyDelayScanGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.estimator.finish_scan(self.token, false);
        }
    }
}

fn flush_scan(
    observer: &Arc<dyn ExternalMetricsObserver>,
    state: &mut EstimatorState,
    active: ActiveScan,
    complete: bool,
) {
    if !complete {
        for (_, confirmed_at) in &active.candidates {
            let _ = confirmed_at;
            record_sample(
                observer,
                active.world,
                active.phase,
                ReplyDelaySampleOutcome::CoverageUnknown,
            );
        }
        // 不完整扫描也可能已提交命令；记住这些帖子，后续命令不能替代首次确认终点。
        if let Some(topic) = state.topics.get_mut(&active.auction_id) {
            for post_id in active
                .candidates
                .iter()
                .map(|(post_id, _)| post_id)
                .chain(active.overflow_ids.iter())
            {
                remember_seen_import(&mut topic.seen_imports, post_id);
            }
        }
        return;
    }
    let was_evicted = state.evicted.remove(&active.auction_id);
    if was_evicted {
        state.eviction_order.retain(|evicted| *evicted != active.auction_id);
    }
    let baseline = state.topics.get(&active.auction_id);
    if baseline.is_none() {
        let outcome = if was_evicted {
            ReplyDelaySampleOutcome::StateEvicted
        } else {
            ReplyDelaySampleOutcome::MissingBaseline
        };
        for _ in &active.candidates {
            record_sample(observer, active.world, active.phase, outcome);
        }
    } else if let Some(baseline) = baseline {
        for (post_id, confirmed_at) in &active.candidates {
            match sample_outcome(&baseline.scan, &active, post_id, *confirmed_at) {
                SampleResult::NotNew => {}
                SampleResult::Observed { lower, midpoint, upper } => {
                    observer.observe(ExternalMetricsEvent::ForumReplyDelayEstimate {
                        world: active.world,
                        phase: active.phase,
                        method: ReplyDelayMethod::PollWindow,
                        lower,
                        midpoint,
                        upper,
                    });
                    record_sample(
                        observer,
                        active.world,
                        active.phase,
                        ReplyDelaySampleOutcome::Observed,
                    );
                }
                SampleResult::StateEvicted => record_sample(
                    observer,
                    active.world,
                    active.phase,
                    ReplyDelaySampleOutcome::StateEvicted,
                ),
                SampleResult::ClockInvalid => record_sample(
                    observer,
                    active.world,
                    active.phase,
                    ReplyDelaySampleOutcome::ClockInvalid,
                ),
            }
        }
    }
    let mut seen_imports =
        state.topics.remove(&active.auction_id).map(|topic| topic.seen_imports).unwrap_or_default();
    for post_id in
        active.candidates.iter().map(|(post_id, _)| post_id).chain(active.overflow_ids.iter())
    {
        remember_seen_import(&mut seen_imports, post_id);
    }
    let scan = ScanWindow {
        started: active.started,
        post_ids: active.post_ids,
        truncated: active.truncated,
    };
    state.next_touch = state.next_touch.wrapping_add(1).max(1);
    let touched = state.next_touch;
    state.topics.insert(active.auction_id, TopicBaseline { scan, seen_imports, touched });
    if state.topics.len() > MAX_TOPICS
        && let Some(oldest) = state
            .topics
            .iter()
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(auction_id, _)| *auction_id)
        && state.topics.remove(&oldest).is_some()
    {
        remember_evicted(state, oldest);
    }
}

fn remember_seen_import(seen_imports: &mut HashSet<String>, post_id: &str) {
    if seen_imports.len() < MAX_POST_IDS_PER_SCAN {
        seen_imports.insert(post_id.to_owned());
    }
}

enum SampleResult {
    NotNew,
    StateEvicted,
    Observed {
        lower: std::time::Duration,
        midpoint: std::time::Duration,
        upper: std::time::Duration,
    },
    ClockInvalid,
}

fn sample_outcome(
    baseline: &ScanWindow,
    active: &ActiveScan,
    post_id: &str,
    confirmed_at: Instant,
) -> SampleResult {
    // 基线被有界状态截断时，无法证明帖子此前不可见；整轮不采样。
    if baseline.truncated {
        return SampleResult::StateEvicted;
    }
    // 已在上次完整扫描中可见的帖子不是新回帖，不做样本，也不伪造零延迟。
    if baseline.post_ids.contains(post_id) {
        return SampleResult::NotNew;
    }
    let Some(lower) = confirmed_at.checked_duration_since(active.completed) else {
        return SampleResult::ClockInvalid;
    };
    let Some(upper) = confirmed_at.checked_duration_since(baseline.started) else {
        return SampleResult::ClockInvalid;
    };
    let Some(width) = upper.checked_sub(lower) else {
        return SampleResult::ClockInvalid;
    };
    let Some(midpoint) = lower.checked_add(width / 2) else {
        return SampleResult::ClockInvalid;
    };
    SampleResult::Observed { lower, midpoint, upper }
}

fn lock_state(state: &Mutex<EstimatorState>) -> std::sync::MutexGuard<'_, EstimatorState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn record_sample(
    observer: &Arc<dyn ExternalMetricsObserver>,
    world: ForumWorld,
    phase: ReplyDelayPhase,
    outcome: ReplyDelaySampleOutcome,
) {
    observer.observe(ExternalMetricsEvent::ForumReplyDelaySample {
        world,
        phase,
        method: ReplyDelayMethod::PollWindow,
        outcome,
    });
}

fn remember_evicted(state: &mut EstimatorState, auction_id: AuctionId) {
    if state.evicted.insert(auction_id) {
        state.eviction_order.push_back(auction_id);
        while state.eviction_order.len() > MAX_EVICTED_TOPICS {
            if let Some(oldest) = state.eviction_order.pop_front() {
                state.evicted.remove(&oldest);
            }
        }
    }
}

fn forum_world(world: SettlementWorld) -> ForumWorld {
    match world {
        SettlementWorld::Main => ForumWorld::Main,
        SettlementWorld::Isekai => ForumWorld::Isekai,
    }
}

fn reply_delay_phase(phase: ForumReplyPhase) -> ReplyDelayPhase {
    match phase {
        ForumReplyPhase::Normal => ReplyDelayPhase::Normal,
        ForumReplyPhase::Final => ReplyDelayPhase::Final,
        ForumReplyPhase::Unknown => ReplyDelayPhase::Unknown,
    }
}
