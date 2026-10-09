//! 因果意图先落盘，再确认 applied；恢复只重放固定身份，不生成骰子或请求。

use super::{
    facts::{Fact, MutationData, WorldMutation},
    format::{self, Body},
    session::{Session, store_fault},
};
use crate::fault::Fault;

/// SQL / 重建解释器边界；产品未登记属性 schema 时不提供实现。
pub trait WorldPort {
    fn registered(&self, kind: &str, version: u32) -> bool;
    /// # Errors
    /// applied 身份和 hash 失配必须返回 corrupt。
    fn applied(&self, id: &str, hash: &str) -> Result<bool, Fault>;
    /// 条件检查、领域更新和 applied 标记必须同一个 SQLite 事务。
    /// # Errors
    /// 未提交 / 提交不确定 / 条件冲突都保留持久意图并冻结会话推进。
    fn apply(&mut self, mutation: &WorldMutation, hash: &str) -> Result<(), Fault>;
}
fn mutations(body: &Body) -> Result<Vec<WorldMutation>, Fault> {
    let Body::System { code, data, .. } = body else {
        return Err(Fault::bad_request());
    };
    let fact = Fact::decode(code, data).map_err(store_fault)?;
    match fact {
        Fact::SettlementPlanned { mutations, .. } => Ok(mutations),
        Fact::SceneAdvanced { mutation_id, .. } | Fact::SessionEnded { mutation_id, .. } => {
            Ok(vec![WorldMutation {
                mutation_id,
                data: MutationData {
                    version: 1,
                    kind: code.clone(),
                    payload: data.clone(),
                },
            }])
        }
        _ => Err(Fault::new(
            "engine.invalid-phase",
            "此事实不是已登记世界意图",
        )),
    }
}
fn mutation_hash(mutation: &WorldMutation) -> Result<String, Fault> {
    Ok(format::hash(
        format::line(mutation).map_err(store_fault)?.as_bytes(),
    ))
}
impl Session {
    /// 验证解释器后强同步意图；SQL 失败不删历史、不发布成功状态。
    /// # Errors
    /// 未登记操作拒绝在落盘前；SQL 失败保留 needsRecovery，阻止后续追加。
    pub fn commit_world(&mut self, body: Body, port: &mut dyn WorldPort) -> Result<u64, Fault> {
        self.writable()?;
        let mutations = mutations(&body)?;
        if mutations
            .iter()
            .any(|m| !port.registered(&m.data.kind, m.data.version))
        {
            return Err(Fault::new("engine.invalid-phase", "世界操作解释器未登记"));
        }
        // append_intent 只耐久保存意图，事件确认留到 SQL 成功之后。
        let seq = self.append_intent(body)?;
        self.pending_world = true;
        self.needs_recovery = true;
        for mutation in mutations {
            let hash = mutation_hash(&mutation)?;
            if !port.applied(&mutation.mutation_id, &hash)? {
                port.apply(&mutation, &hash)?;
            }
        }
        self.pending_world = false;
        self.needs_recovery = self.pending_history;
        self.confirm_intent(seq)?;
        Ok(seq)
    }
    /// 重开核验 applied 后，只补缺失固定操作；没有解释器保持只读 pending。
    /// # Errors
    /// 冲突 / 未登记 / SQL 失败禁止继续推进，已成功项不重做。
    pub fn recover_world(&mut self, port: &mut dyn WorldPort) -> Result<(), Fault> {
        if self.read_only || self.frozen {
            return Err(Fault::new(
                "engine.invalid-phase",
                "未知或不确定记录不能修改世界",
            ));
        }
        self.pending_world = true;
        self.needs_recovery = true;
        if self.pending_history {
            return Err(Fault::new(
                "engine.invalid-phase",
                "先核验因果控制再恢复世界意图",
            ));
        }
        let sequences = self.path_at(self.history_revision)?;
        for seq in sequences {
            let record = self.known(seq).map_err(store_fault)?;
            if !matches!(&record.body,Body::System{code,..} if matches!(code.as_str(),"settlementPlanned"|"sceneAdvanced"|"sessionEnded"))
            {
                continue;
            }
            for mutation in mutations(&record.body)? {
                let hash = mutation_hash(&mutation)?;
                if port.applied(&mutation.mutation_id, &hash)? {
                    continue;
                }
                if !port.registered(&mutation.data.kind, mutation.data.version) {
                    return Err(Fault::new("engine.invalid-phase", "待恢复操作解释器未登记"));
                }
                port.apply(&mutation, &hash)?;
            }
        }
        self.pending_world = false;
        self.needs_recovery = self.pending_history;
        for seq in self.pending_confirmations() {
            self.confirm_intent(seq)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
