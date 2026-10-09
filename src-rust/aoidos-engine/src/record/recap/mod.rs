//! 摘要候选只覆盖冻结且不在逐字尾部的来源；状态变化后不落库。

use super::{
    format::{self, Body, RecapOrigin, Record},
    projection::PromptPlan,
    session::{Session, store_fault},
    working_set::WorkingSet,
};
use crate::fault::Fault;
use sha2::{Digest, Sha256};

/// 不透明候选同时绑定当前输入、有效分支、版本与读取边界。
pub struct RecapCandidate {
    session: String,
    epoch: String,
    next_seq: u64,
    history: u64,
    static_hash: String,
    grammar: u32,
    projection: u32,
    from: u64,
    through: u64,
    source_hash: String,
    estimator: u32,
    source: String,
}
impl RecapCandidate {
    /// 有界、未改写的源行，调用方在请求副本中进行结构转义。
    pub fn source(&self) -> &str {
        &self.source
    }
}
impl Session {
    /// 冻结摘要来源与生成基线；候选最多 64 块 / 256 KiB。
    /// # Errors
    /// 过期工作集、重叠旧摘要、逐字尾部或非法来源在请求前拒绝。
    pub fn prepare_recap(
        &self,
        workset: &WorkingSet,
        plan: &PromptPlan,
        from: u64,
        through: u64,
    ) -> Result<RecapCandidate, Fault> {
        self.writable()?;
        if self.in_flight().is_some() {
            return Err(Fault::busy());
        }
        if plan.working_set_hash != workset.fingerprint()
            || workset.session_id != self.header().session_id
            || workset.epoch != self.view_epoch()
            || workset.boundary != self.index.keys().next_back().copied().unwrap_or(0)
            || !matches!(plan.estimator_version, 1 | 2)
        {
            return Err(Fault::bad_request());
        }
        let path = self.path_at(self.history_revision())?;
        if !path.contains(&from) || !path.contains(&through) || through < from {
            return Err(Fault::bad_request());
        }
        let mut source = String::new();
        let mut count = 0;
        let mut digest = Sha256::new();
        for seq in path
            .into_iter()
            .filter(|seq| *seq >= from && *seq <= through)
        {
            let parsed = self.read(seq).map_err(store_fault)?;
            if matches!(
                parsed.body.as_ref().map(|r| &r.body),
                Some(Body::Recap { .. })
            ) {
                continue;
            }
            if plan.included.contains(&seq) && !plan.folded.contains(&seq) {
                return Err(Fault::bad_request());
            }
            count += 1;
            if count > 64 || source.len() + parsed.raw.len() > 256 * 1024 {
                return Err(Fault::bad_request());
            }
            digest.update(parsed.raw.as_bytes());
            source.push_str(&parsed.raw);
        }
        let source_hash = format!("sha256:{:x}", digest.finalize());
        let validation = Record {
            seq: self.next_seq,
            created_at: format::now(),
            branch_seq: (self.history_revision() > 0).then_some(self.history_revision()),
            body: Body::Recap {
                from_seq: from,
                through_seq: through,
                text: "候选校验".into(),
                source_hash: source_hash.clone(),
                estimator_version: plan.estimator_version,
                origin: RecapOrigin::Manual,
            },
        };
        validation.validate().map_err(store_fault)?;
        self.check_references(&validation).map_err(store_fault)?;
        Ok(RecapCandidate {
            session: self.header().session_id.clone(),
            epoch: self.view_epoch().into(),
            next_seq: self.next_seq,
            history: self.history_revision(),
            static_hash: self.header().static_prefix_hash.clone(),
            grammar: self.header().grammar_version,
            projection: self.header().projection_version,
            from,
            through,
            source_hash,
            estimator: plan.estimator_version,
            source,
        })
    }
    /// 在同一单写者临界区重新核验后提交，不重用过期候选的身份。
    /// # Errors
    /// 输入、分支或版本变化、正文非法、存储失败均拒绝并保留原记录。
    pub(super) fn validate_recap_candidate(&self, candidate: &RecapCandidate) -> Result<(), Fault> {
        self.writable()?;
        if self.in_flight().is_some()
            || candidate.session != self.header().session_id
            || candidate.epoch != self.view_epoch()
            || candidate.next_seq != self.next_seq
            || candidate.history != self.history_revision()
            || candidate.static_hash != self.header().static_prefix_hash
            || candidate.grammar != self.header().grammar_version
            || candidate.projection != self.header().projection_version
        {
            return Err(Fault::new("engine.invalid-phase", "摘要候选已过期"));
        }
        Ok(())
    }
    /// 核验后才分配身份并提交，失败不推进摘要覆盖边界。
    /// # Errors
    /// 候选过期、正文非法或存储失败时保留原记录。
    pub fn commit_recap(
        &mut self,
        candidate: RecapCandidate,
        text: String,
        origin: RecapOrigin,
    ) -> Result<u64, Fault> {
        self.validate_recap_candidate(&candidate)?;
        let body = Body::Recap {
            from_seq: candidate.from,
            through_seq: candidate.through,
            text,
            source_hash: candidate.source_hash,
            estimator_version: candidate.estimator,
            origin,
        };
        let seq = self.reserve()?;
        self.commit(Record {
            seq,
            created_at: format::now(),
            branch_seq: (self.history_revision() > 0).then_some(self.history_revision()),
            body,
        })?;
        Ok(seq)
    }
}

#[cfg(test)]
mod tests;
