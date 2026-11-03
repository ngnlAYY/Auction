//! 待上传项逐条校验；坏记录和目的地变化只保留告警，不阻断其他恢复点。

use uuid::Uuid;

use super::super::{
    journal::Journal,
    model::{FORMAT, Outcome, RunReceipt},
};
use super::{
    model::{PENDING_FORMAT, Pending},
    state::{State, name},
};
use crate::app::progress;

impl State {
    pub(super) async fn queue(
        &self,
        current: &mut RunReceipt,
        destination: &str,
        journal: &Journal,
    ) -> anyhow::Result<Vec<Pending>> {
        let mut queue = Vec::new();
        for file in self.pending.names()? {
            tokio::task::yield_now().await;
            // 本次项目已优先尝试；即使失败也留到下次运行，不在同一预算内重复尝试。
            if !file.ends_with(".json") || file == name(current.run_id) {
                continue;
            }
            match self.load_item(&file, current, destination, journal) {
                Ok(pending) => queue.push(pending),
                Err(reason) => {
                    let run_id = file
                        .strip_suffix(".json")
                        .and_then(|value| Uuid::parse_str(value).ok())
                        .filter(|run_id| !run_id.is_nil() && file == name(*run_id));
                    current.offsite.skip_pending(run_id, reason);
                    progress(format_args!("warning={reason} offsite_run_id={run_id:?}"));
                }
            }
        }
        queue.sort_by_key(|item| (item.started_at, item.run_id));
        Ok(queue)
    }

    fn load_item(
        &self,
        file: &str,
        current: &RunReceipt,
        destination: &str,
        journal: &Journal,
    ) -> Result<Pending, &'static str> {
        let bytes = self.pending.read(file).map_err(|_| "pending_unreadable")?;
        let pending: Pending =
            serde_json::from_slice(&bytes).map_err(|_| "pending_record_invalid")?;
        if pending.format != PENDING_FORMAT
            || pending.run_id.is_nil()
            || file != name(pending.run_id)
        {
            return Err("pending_record_invalid");
        }
        if pending.destination_sha256 != destination {
            return Err("pending_destination_changed");
        }
        if pending.output_dir != current.output_dir
            || pending.database_name_sha256 != current.database_name_sha256
            || !pending.archive.verified
        {
            return Err("pending_identity_mismatch");
        }
        let source: RunReceipt = serde_json::from_slice(
            &journal.receipts.read(&name(pending.run_id)).map_err(|_| "pending_source_invalid")?,
        )
        .map_err(|_| "pending_source_invalid")?;
        if source.format != FORMAT
            || source.outcome == Outcome::Running
            || source.finished_at.is_none()
        {
            return Err("pending_source_invalid");
        }
        let archive = source.archive.as_ref().ok_or("pending_source_invalid")?;
        if source.run_id != pending.run_id
            || source.started_at != pending.started_at
            || source.output_dir != pending.output_dir
            || source.database_name_sha256 != pending.database_name_sha256
            || !source.offsite.enabled
            || !archive.verified
            || archive != &pending.archive
            || archive.archive_file_name != source.archive_name()
            || archive.manifest_file_name != format!("{}.manifest", source.archive_name())
        {
            return Err("pending_source_mismatch");
        }
        Ok(pending)
    }
}
