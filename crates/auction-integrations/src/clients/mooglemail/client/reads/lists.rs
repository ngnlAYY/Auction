//! 三种真实邮箱列表的有界完整扫描；列表身份与页数一起缓存，缺页不会伪装为完整。

use std::{collections::HashSet, mem::size_of};

use tokio::sync::OnceCell;

use super::*;
use crate::clients::mooglemail::parser::{
    MoogleMailParser, MoogleMailSentSummary, MoogleMailSummary,
};

type ListCell<T> = Arc<OnceCell<Arc<T>>>;

#[derive(Default)]
pub(super) struct ListCells {
    sent: StdMutex<HashMap<usize, ListCell<MoogleMailSentList>>>,
    incoming: StdMutex<HashMap<(bool, usize, u64), ListCell<MoogleMailIncomingList>>>,
}

pub(crate) struct MoogleMailSentList {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) pages_scanned: usize,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

pub(crate) struct MoogleMailIncomingList {
    pub(crate) summaries: Vec<MoogleMailSummary>,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

impl MoogleMailClient {
    /// 同一轮次完整 Sent 摘要只解析一次；履约逐 MID 读取不重复扫描全网列表。
    pub(crate) async fn observe_sent_list(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
    ) -> Result<Arc<MoogleMailSentList>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let cell = {
            let mut cells = round.epoch.lists.sent.lock().map_err(|_| read_unavailable(world))?;
            if cells.len() >= 8 && !cells.contains_key(&maximum_pages) {
                return Err(read_unavailable(world));
            }
            Arc::clone(cells.entry(maximum_pages).or_default())
        };
        let list = cell
            .get_or_try_init(|| async {
                let mut summaries = Vec::new();
                let mut seen = HashSet::new();
                let mut pages_scanned = 0;
                let mut complete = false;
                let mut bytes = self.retain_list_bytes(0, world)?;
                for page in 0..maximum_pages {
                    let target = self.sent_page(world, page)?;
                    let document = self.get_in_round(&target, cookie, round).await?;
                    let parsed = self
                        .diagnose_parse(
                            &target,
                            &document,
                            MoogleMailParser.scan_sent_page(&document, world, page, &mut seen),
                        )
                        .await?;
                    let previous_capacity = summaries.capacity();
                    let payload_bytes = parsed
                        .summaries
                        .iter()
                        .map(|summary| {
                            summary.external_mail_id.capacity()
                                + summary.recipient.capacity()
                                + summary.subject.capacity()
                        })
                        .sum::<usize>();
                    summaries.extend(parsed.summaries);
                    // 逐页计费，不能先积累整箱摘要后才检查内存上限。
                    bytes.merge(self.retain_list_bytes(
                        payload_bytes
                            + (summaries.capacity() - previous_capacity)
                                * size_of::<MoogleMailSentSummary>(),
                        world,
                    )?);
                    pages_scanned += 1;
                    if !parsed.has_next_page {
                        complete = true;
                        break;
                    }
                }
                self.check_read_round(cookie, round).await?;
                Ok::<_, MoogleMailError>(Arc::new(MoogleMailSentList {
                    summaries,
                    pages_scanned,
                    complete,
                    _bytes: bytes,
                }))
            })
            .await?;
        self.check_read_round(cookie, round).await?;
        Ok(Arc::clone(list))
    }

    /// Read Mail 只返回历史摘要；新入站业务必须来自 Inbox 窗口或既有持久待读账本。
    /// 整个分页过程与改变列表成员关系的详情 GET 互斥，而不占用 compose/资产 guard。
    pub(crate) async fn observe_incoming_list(
        &self,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        maximum_pages: usize,
        read_mail: bool,
    ) -> Result<Arc<MoogleMailIncomingList>, MoogleMailError> {
        let world = round.key.world;
        self.check_read_round(cookie, round).await?;
        validate_pages(maximum_pages, world)?;
        let _list_guard = Arc::clone(&round.scope.incoming).read_owned().await;
        let membership = round.scope.membership_revision.load(Ordering::Acquire);
        let key = (read_mail, maximum_pages, membership);
        let cell = {
            let mut cells =
                round.epoch.lists.incoming.lock().map_err(|_| read_unavailable(world))?;
            cells.retain(|(_, _, revision), _| *revision == membership);
            if cells.len() >= 8 && !cells.contains_key(&key) {
                return Err(read_unavailable(world));
            }
            Arc::clone(cells.entry(key).or_default())
        };
        let list = cell
            .get_or_try_init(|| async {
                let mut summaries = Vec::new();
                let mut seen = HashSet::new();
                let mut complete = false;
                let mut bytes = self.retain_list_bytes(0, world)?;
                for page in 0..maximum_pages {
                    let target = if read_mail {
                        self.read_mail_page(world, page)?
                    } else {
                        self.inbox_page(world, page)?
                    };
                    let document = self.get_in_round(&target, cookie, round).await?;
                    let parsed = if read_mail {
                        MoogleMailParser.scan_read_mail_page(&document, world, page, &mut seen)
                    } else {
                        MoogleMailParser.scan_inbox_page(&document, world, page, &mut seen)
                    };
                    let parsed = self.diagnose_parse(&target, &document, parsed).await?;
                    let previous_capacity = summaries.capacity();
                    let payload_bytes = parsed
                        .summaries
                        .iter()
                        .map(|summary| {
                            summary.external_mail_id.capacity()
                                + summary.sender.capacity()
                                + summary.subject.capacity()
                        })
                        .sum::<usize>();
                    summaries.extend(parsed.summaries);
                    bytes.merge(self.retain_list_bytes(
                        payload_bytes
                            + (summaries.capacity() - previous_capacity)
                                * size_of::<MoogleMailSummary>(),
                        world,
                    )?);
                    if !parsed.has_next_page {
                        complete = true;
                        break;
                    }
                }
                self.check_read_round(cookie, round).await?;
                Ok::<_, MoogleMailError>(Arc::new(MoogleMailIncomingList {
                    summaries,
                    complete,
                    _bytes: bytes,
                }))
            })
            .await?;
        self.check_read_round(cookie, round).await?;
        Ok(Arc::clone(list))
    }

    fn retain_list_bytes(
        &self,
        bytes: usize,
        world: MoogleMailWorld,
    ) -> Result<OwnedSemaphorePermit, MoogleMailError> {
        let bytes = u32::try_from(bytes).map_err(|_| read_unavailable(world))?;
        Arc::clone(&self.reads.bytes)
            .try_acquire_many_owned(bytes)
            .map_err(|_| read_unavailable(world))
    }
}

fn validate_pages(maximum_pages: usize, world: MoogleMailWorld) -> Result<(), MoogleMailError> {
    if !(1..=100).contains(&maximum_pages) {
        return Err(MoogleMailError::new(
            MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::Configuration,
            false,
        ));
    }
    Ok(())
}
