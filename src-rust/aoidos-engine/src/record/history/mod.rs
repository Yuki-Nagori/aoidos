//! 因果分支只在 applied 核验后切换；物理历史永远不删除。

use super::{
    facts::Fact,
    format::Body,
    session::{Session, store_fault},
};
use crate::fault::Fault;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// 世界重建必须由已登记解释器执行；022 不提供任意目标的产品控制入口。
pub trait HistoryPort {
    /// 同时验证目标为可重放检查点，不能仅因为 seq 存在就允许 rewind。
    /// # Errors
    /// 缺解释器 / 非法目标 / pending 世界意图拒绝。
    fn verify_target(&self, session: &Session, target: u64) -> Result<(), Fault>;
    /// 已耐久控制恢复时按其父路径核验；不能把当前 needsRecovery 当成新控制授权。
    /// # Errors
    /// 未登记的恢复解释器或非法父路径 / 边界拒绝；旧实现保持原核验策略。
    fn verify_replay_target(
        &self,
        session: &Session,
        _parent: u64,
        target: u64,
    ) -> Result<(), Fault> {
        self.verify_target(session, target)
    }
    fn applied(&self, operation: &str, hash: &str) -> Result<bool, Fault>;
    /// 重建当前状态与 applied 控制标记必须同 SQLite 事务。
    /// # Errors
    /// 条件冲突 / SQL 不确定结果保留 pending，不启动付费生成。
    fn rebuild(
        &mut self,
        session: &Session,
        path: &[u64],
        operation: &str,
        hash: &str,
    ) -> Result<(), Fault>;
}
impl Session {
    /// 通过已 applied 的父控制链计算有效物理 seq；控制本身不进入 prefix hash。
    /// # Errors
    /// 缺引用、循环 / 非祖先控制或 hash 不符返回 corrupt。
    pub fn path_at(&self, control: u64) -> Result<Vec<u64>, Fault> {
        let mut controls = Vec::new();
        let mut current = control;
        while current > 0 {
            let record = self
                .read(current)
                .map_err(store_fault)?
                .body
                .ok_or_else(Fault::bad_request)?;
            let branch_seq = record.branch_seq;
            let Body::System { code, data, .. } = record.body else {
                return Err(Fault::bad_request());
            };
            let Fact::HistoryFork {
                parent_control_seq,
                target_seq,
                prefix_hash,
                ..
            } = Fact::decode(&code, &data).map_err(store_fault)?
            else {
                return Err(Fault::bad_request());
            };
            if branch_seq.unwrap_or(0) != parent_control_seq
                || parent_control_seq >= current
                || target_seq >= current
            {
                return Err(Fault::bad_request());
            }
            controls.push((current, parent_control_seq, target_seq, prefix_hash));
            current = parent_control_seq;
        }
        let mut path = self
            .index
            .iter()
            .filter(|(_, index)| index.branch_seq.is_none() && !index.control)
            .map(|(&seq, _)| seq)
            .collect::<Vec<_>>();
        let mut branch = 0;
        for (control, parent, target, hash) in controls.into_iter().rev() {
            if parent != branch || !path.contains(&target) {
                return Err(Fault::bad_request());
            }
            path.retain(|seq| *seq <= target);
            if self.prefix_hash(&path)? != hash {
                return Err(Fault::new("store.corrupt", "因果前缀摘要不一致"));
            }
            path.extend(
                self.index
                    .iter()
                    .filter(|(_, index)| index.branch_seq == Some(control) && !index.control)
                    .map(|(&seq, _)| seq),
            );
            branch = control;
        }
        path.sort_unstable();
        Ok(path)
    }
    fn prefix_hash(&self, path: &[u64]) -> Result<String, Fault> {
        let mut digest = Sha256::new();
        for &seq in path {
            let record = self.read(seq).map_err(store_fault)?;
            if matches!(record.body.as_ref().map(|r|&r.body),Some(Body::System{code,..}) if code=="historyFork")
            {
                continue;
            }
            digest.update(record.raw.as_bytes());
        }
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
    /// 先核验并强同步控制，再重建 SQL，最后更新有效路径 / epoch。
    /// # Errors
    /// 解释器缺失或重建失败保留原路径与 pending，不把物理控制当 applied。
    pub fn commit_history_fork(
        &mut self,
        body: Body,
        port: &mut dyn HistoryPort,
    ) -> Result<u64, Fault> {
        self.writable()?;
        let Body::System { code, data, .. } = &body else {
            return Err(Fault::bad_request());
        };
        let Fact::HistoryFork {
            operation_id,
            parent_control_seq,
            target_seq,
            prefix_hash,
            ..
        } = Fact::decode(code, data).map_err(store_fault)?
        else {
            return Err(Fault::bad_request());
        };
        if parent_control_seq != self.history_revision {
            return Err(Fault::bad_request());
        }
        let mut path = self.path_at(parent_control_seq)?;
        if !path.contains(&target_seq) {
            return Err(Fault::bad_request());
        }
        path.retain(|seq| *seq <= target_seq);
        if self.prefix_hash(&path)? != prefix_hash {
            return Err(Fault::bad_request());
        }
        port.verify_target(self, target_seq)?;
        let seq = self.append_intent(body)?;
        self.pending_history = true;
        self.needs_recovery = true;
        let content_hash = self.index[&seq].hash.clone();
        if !port.applied(&operation_id, &content_hash)? {
            port.rebuild(self, &path, &operation_id, &content_hash)?;
        }
        self.history_revision = seq;
        self.effective = Some(path.iter().copied().collect::<BTreeSet<_>>());
        self.view_epoch = uuid::Uuid::new_v4().to_string();
        self.pending_history = false;
        self.needs_recovery = self.pending_world;
        self.confirm_intent(seq)?;
        Ok(seq)
    }
    /// 重开只恢复既有控制，不追加新 operation 或再掷骰。
    /// # Errors
    /// 未登记解释器或 applied 内容冲突保持 pending。
    pub fn recover_history(&mut self, port: &mut dyn HistoryPort) -> Result<(), Fault> {
        if self.read_only || self.frozen {
            return Err(Fault::new(
                "engine.invalid-phase",
                "未知或不确定记录不能重建世界",
            ));
        }
        self.pending_history = true;
        self.needs_recovery = true;
        let mut active = 0;
        for seq in self.index.keys().copied().collect::<Vec<_>>() {
            let record = self.known(seq).map_err(store_fault)?;
            let Body::System { code, data, .. } = record.body else {
                continue;
            };
            if code != "historyFork" {
                continue;
            }
            let (operation_id, parent_control_seq, target_seq) = fork_identity(&code, &data)?;
            if parent_control_seq != active {
                return Err(Fault::new("store.corrupt", "控制父路径不一致"));
            }
            let path = self
                .path_at(seq)?
                .into_iter()
                .filter(|position| *position < seq)
                .collect::<Vec<_>>();
            let content_hash = self.index[&seq].hash.clone();
            if !port.applied(&operation_id, &content_hash)? {
                port.verify_replay_target(self, parent_control_seq, target_seq)?;
                port.rebuild(self, &path, &operation_id, &content_hash)?;
            }
            active = seq;
        }
        self.history_revision = active;
        self.effective = Some(self.path_at(active)?.into_iter().collect());
        self.view_epoch = uuid::Uuid::new_v4().to_string();
        self.pending_history = false;
        self.needs_recovery = self.pending_world;
        for seq in self.pending_confirmations() {
            if self.index[&seq].control {
                self.confirm_intent(seq)?;
            }
        }
        Ok(())
    }
}

fn fork_identity(code: &str, data: &serde_json::Value) -> Result<(String, u64, u64), Fault> {
    match Fact::decode(code, data).map_err(store_fault)? {
        Fact::HistoryFork {
            operation_id,
            parent_control_seq,
            target_seq,
            ..
        } => Ok((operation_id, parent_control_seq, target_seq)),
        _ => Err(Fault::bad_request()),
    }
}

#[cfg(test)]
mod tests;
