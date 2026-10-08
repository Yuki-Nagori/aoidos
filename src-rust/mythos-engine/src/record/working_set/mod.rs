//! 从单写者读取有界投影工作集；旧摘要来源逐行核验，不整体载入内存。

use super::{
    format::{self, Body, Header, Parsed},
    projection::{RecordView, SeqRange},
    session::{Session, store_fault},
};
use crate::fault::Fault;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_WORKSET_RECORDS: usize = 256;
pub const MAX_WORKSET_BYTES: usize = 8 * 1024 * 1024;

/// 只有 Session 能构造来源证据；对外借出不可变记录，不接受 Webview 身份。
pub struct WorkingSet {
    pub(super) session_id: String,
    pub(super) epoch: String,
    pub(super) boundary: u64,
    header: Header,
    records: Vec<Parsed>,
    omitted: Vec<SeqRange>,
    proofs: BTreeMap<u64, String>,
    needs_recovery: bool,
}
impl WorkingSet {
    pub(super) fn fingerprint(&self) -> String {
        fingerprint(&self.header, &self.records)
    }
    pub fn view(&self) -> RecordView<'_> {
        RecordView {
            header: &self.header,
            records: &self.records,
            needs_recovery: self.needs_recovery,
            omitted_ranges: &self.omitted,
            verified_recaps: &self.proofs,
        }
    }
    pub fn records(&self) -> &[Parsed] {
        &self.records
    }
}
pub(super) fn fingerprint(header: &Header, records: &[Parsed]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(header.session_id.as_bytes());
    hash.update(header.static_prefix_hash.as_bytes());
    for record in records {
        hash.update(record.raw.as_bytes());
    }
    format!("sha256:{:x}", hash.finalize())
}
impl Session {
    /// 最多 256 块 / 8 MiB 原行；必需输入先读，旧来源以流式 hash 核验。
    /// # Errors
    /// 损坏或必需块超过工作集上限拒绝；pending / 未知记录由纯投影门禁拒绝。
    pub fn projection_working_set(&self) -> Result<WorkingSet, Fault> {
        let eligible = |seq: u64| {
            let index = &self.index[&seq];
            self.is_read_only()
                || (!index.control
                    && self.effective.as_ref().map_or(
                        index.branch_seq.unwrap_or(0) == self.history_revision(),
                        |path| {
                            path.contains(&seq) || index.branch_seq == Some(self.history_revision())
                        },
                    ))
        };
        let player = self
            .index
            .iter()
            .rev()
            .find(|(seq, index)| eligible(**seq) && index.kind == "playerSpeech")
            .map(|(seq, _)| *seq);
        let latest = self.index.keys().rev().copied().find(|seq| eligible(*seq));
        let mut chosen = BTreeMap::new();
        let mut bytes = 0;
        let mut proofs = BTreeMap::new();
        let mut recap_count = 0;
        let mut recap_bytes = 0;
        let recaps = self
            .index
            .iter()
            .rev()
            .filter(|(seq, index)| eligible(**seq) && index.kind == "recap")
            .map(|(seq, _)| *seq);
        let order = [latest, player].into_iter().flatten().chain(recaps).chain(
            self.index
                .keys()
                .rev()
                .copied()
                .filter(|seq| eligible(*seq)),
        );
        for seq in order {
            if chosen.contains_key(&seq) {
                continue;
            }
            let index = &self.index[&seq];
            if chosen.len() == MAX_WORKSET_RECORDS || bytes + index.len > MAX_WORKSET_BYTES {
                continue;
            }
            if index.kind == "recap" && (recap_count == 16 || recap_bytes + index.len > 1024 * 1024)
            {
                continue;
            }
            let parsed = self.read(seq).map_err(store_fault)?;
            if let Some(record) = &parsed.body
                && matches!(record.body, Body::Recap { .. })
                && !parsed.read_only
            {
                self.check_references(record).map_err(store_fault)?;
                proofs.insert(seq, format::hash(parsed.raw.as_bytes()));
                recap_count += 1;
                recap_bytes += parsed.raw.len();
            }
            bytes += parsed.raw.len();
            chosen.insert(seq, parsed);
        }
        let selected = chosen.keys().copied().collect::<BTreeSet<_>>();
        let mut omitted: Vec<SeqRange> = Vec::new();
        let mut previous_omitted = false;
        for seq in self.index.keys().copied().filter(|seq| eligible(*seq)) {
            if selected.contains(&seq) {
                previous_omitted = false;
                continue;
            }
            if previous_omitted {
                omitted
                    .last_mut()
                    .expect("previous omitted range exists")
                    .through = seq;
            } else {
                omitted.push(SeqRange {
                    from: seq,
                    through: seq,
                });
            }
            previous_omitted = true;
        }
        Ok(WorkingSet {
            session_id: self.header().session_id.clone(),
            epoch: self.view_epoch().into(),
            boundary: self.index.keys().next_back().copied().unwrap_or(0),
            header: self.header().clone(),
            records: chosen.into_values().collect(),
            omitted,
            proofs,
            needs_recovery: self.needs_recovery(),
        })
    }
}

#[cfg(test)]
mod tests;
