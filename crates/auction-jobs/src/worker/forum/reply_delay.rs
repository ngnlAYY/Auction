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

use auction_application::external_metrics::{
    ExternalMetricsEvent, ExternalMetricsObserver, ForumWorld, ReplyDelayMethod, ReplyDelayPhase,
    ReplyDelaySampleOutcome,
};
use auction_application::forum::ForumReplyPhase;
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
    seen_imports_truncated: bool,
    touched: u64,
}

struct EstimatorState {
    active: HashMap<AuctionId, ActiveScan>,
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
                active: HashMap::new(),
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
        let mut events = Vec::new();
        if let Some(active) = state.active.remove(&auction_id) {
            events.extend(flush_scan(&mut state, active, false));
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
        state.active.insert(
            auction_id,
            ActiveScan {
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
            },
        );
        drop(state);
        for event in events {
            self.observer.observe(event);
        }
        ReplyDelayScanGuard { estimator: self, auction_id, token, finished: false }
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
        let event = {
            let mut state = lock_state(&self.state);
            if let Some(topic) = state.topics.get(&auction_id) {
                if topic.seen_imports.contains(post_id) {
                    return;
                }
                if topic.seen_imports_truncated {
                    Some(sample_event(
                        forum_world(world),
                        reply_delay_phase(phase),
                        ReplyDelaySampleOutcome::StateEvicted,
                    ))
                } else {
                    observe_active_import(
                        state.active.get_mut(&auction_id),
                        world,
                        phase,
                        post_id,
                        confirmed_at,
                    )
                }
            } else {
                observe_active_import(
                    state.active.get_mut(&auction_id),
                    world,
                    phase,
                    post_id,
                    confirmed_at,
                )
            }
        };
        if let Some(event) = event {
            self.observer.observe(event);
        }
    }

    fn finish_scan(&self, auction_id: AuctionId, token: u64, complete: bool) {
        let events = {
            let mut state = lock_state(&self.state);
            let Some(active) = state.active.remove(&auction_id) else { return };
            if active.token != token {
                state.active.insert(auction_id, active);
                return;
            }
            flush_scan(&mut state, active, complete)
        };
        for event in events {
            self.observer.observe(event);
        }
    }
}
fn observe_active_import(
    active: Option<&mut ActiveScan>,
    world: SettlementWorld,
    phase: ForumReplyPhase,
    post_id: &str,
    confirmed_at: Instant,
) -> Option<ExternalMetricsEvent> {
    let Some(active) = active else {
        return Some(sample_event(
            forum_world(world),
            reply_delay_phase(phase),
            ReplyDelaySampleOutcome::MissingBaseline,
        ));
    };
    if !active.post_ids.contains(post_id) {
        let outcome = if active.truncated {
            ReplyDelaySampleOutcome::StateEvicted
        } else {
            ReplyDelaySampleOutcome::CoverageUnknown
        };
        return Some(sample_event(active.world, active.phase, outcome));
    }
    if active.candidate_ids.contains(post_id) {
        return None;
    }
    if active.candidate_ids.len() >= MAX_CANDIDATES_PER_SCAN {
        return active.overflow_ids.insert(post_id.to_owned()).then(|| {
            sample_event(active.world, active.phase, ReplyDelaySampleOutcome::StateEvicted)
        });
    }
    active.candidate_ids.insert(post_id.to_owned());
    active.candidates.push((post_id.to_owned(), confirmed_at));
    None
}

/// 扫描结束或取消时收口；取消不是零延迟样本。
pub(super) struct ReplyDelayScanGuard<'a> {
    estimator: &'a ReplyDelayEstimator,
    auction_id: AuctionId,
    token: u64,
    finished: bool,
}

impl ReplyDelayScanGuard<'_> {
    pub(super) fn finish(mut self, complete: bool) {
        self.finished = true;
        self.estimator.finish_scan(self.auction_id, self.token, complete);
    }
}

impl Drop for ReplyDelayScanGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.estimator.finish_scan(self.auction_id, self.token, false);
        }
    }
}

fn flush_scan(
    state: &mut EstimatorState,
    active: ActiveScan,
    complete: bool,
) -> Vec<ExternalMetricsEvent> {
    let mut events = Vec::new();
    if !complete {
        for _ in &active.candidates {
            events.push(sample_event(
                active.world,
                active.phase,
                ReplyDelaySampleOutcome::CoverageUnknown,
            ));
        }
        if let Some(topic) = state.topics.get_mut(&active.auction_id) {
            for post_id in active
                .candidates
                .iter()
                .map(|(post_id, _)| post_id)
                .chain(active.overflow_ids.iter())
            {
                if !remember_seen_import(&mut topic.seen_imports, post_id) {
                    topic.seen_imports_truncated = true;
                }
            }
        }
        return events;
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
            events.push(sample_event(active.world, active.phase, outcome));
        }
    } else if let Some(baseline) = baseline {
        for (post_id, confirmed_at) in &active.candidates {
            match sample_outcome(&baseline.scan, &active, post_id, *confirmed_at) {
                SampleResult::NotNew => {}
                SampleResult::Observed { lower, midpoint, upper } => {
                    events.push(ExternalMetricsEvent::ForumReplyDelayEstimate {
                        world: active.world,
                        phase: active.phase,
                        method: ReplyDelayMethod::PollWindow,
                        lower,
                        midpoint,
                        upper,
                    });
                    events.push(sample_event(
                        active.world,
                        active.phase,
                        ReplyDelaySampleOutcome::Observed,
                    ));
                }
                SampleResult::StateEvicted => events.push(sample_event(
                    active.world,
                    active.phase,
                    ReplyDelaySampleOutcome::StateEvicted,
                )),
                SampleResult::ClockInvalid => events.push(sample_event(
                    active.world,
                    active.phase,
                    ReplyDelaySampleOutcome::ClockInvalid,
                )),
            }
        }
    }
    let previous = state.topics.remove(&active.auction_id);
    let mut seen_imports_truncated =
        previous.as_ref().is_some_and(|topic| topic.seen_imports_truncated);
    let mut seen_imports = previous.map_or_default(|topic| topic.seen_imports);
    for post_id in
        active.candidates.iter().map(|(post_id, _)| post_id).chain(active.overflow_ids.iter())
    {
        if !remember_seen_import(&mut seen_imports, post_id) {
            seen_imports_truncated = true;
        }
    }
    let scan = ScanWindow {
        started: active.started,
        post_ids: active.post_ids,
        truncated: active.truncated,
    };
    state.next_touch = state.next_touch.wrapping_add(1).max(1);
    let touched = state.next_touch;
    state.topics.insert(
        active.auction_id,
        TopicBaseline { scan, seen_imports, seen_imports_truncated, touched },
    );
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
    events
}

fn remember_seen_import(seen_imports: &mut HashSet<String>, post_id: &str) -> bool {
    if seen_imports.contains(post_id) {
        return true;
    }
    if seen_imports.len() >= MAX_POST_IDS_PER_SCAN {
        return false;
    }
    seen_imports.insert(post_id.to_owned())
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

fn sample_event(
    world: ForumWorld,
    phase: ReplyDelayPhase,
    outcome: ReplyDelaySampleOutcome,
) -> ExternalMetricsEvent {
    ExternalMetricsEvent::ForumReplyDelaySample {
        world,
        phase,
        method: ReplyDelayMethod::PollWindow,
        outcome,
    }
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
