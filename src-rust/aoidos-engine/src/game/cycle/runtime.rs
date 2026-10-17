//! 持久化并恢复场景进入和已完成回合的记忆生命周期事件。

use super::super::domain::{CompletedRound, CompletedRounds};
use crate::{fault::Fault, storage::Storage};
use aoidos_memory::model::{
    BeginSceneProgress, CausalPort, CompleteSceneProgress, CycleEventKind, CycleLifecycleEvent,
    MAX_CYCLE_EVENT_PAGE, SceneNodeKey,
};
use std::sync::{Arc, Mutex};

/// 只负责本地 SQLite 记忆消费；不创建 LLM 请求，也不持有回合 lease。
pub(crate) struct CycleRuntime {
    storage: Arc<Storage>,
    catalog: Arc<super::super::catalog::SceneCatalog>,
    session_id: String,
    run_id: Mutex<Option<String>>,
}

impl CycleRuntime {
    pub(crate) fn new(
        storage: Arc<Storage>,
        catalog: Arc<super::super::catalog::SceneCatalog>,
        session_id: String,
    ) -> Self {
        Self {
            storage,
            catalog,
            session_id,
            run_id: Mutex::new(None),
        }
    }

    pub(crate) fn set_run_id(&self, run_id: String) -> Result<(), Fault> {
        let mut current = self
            .run_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.as_ref().is_some_and(|existing| existing != &run_id) {
            return Err(Fault::bad_request());
        }
        *current = Some(run_id);
        Ok(())
    }

    fn run_id(&self) -> Result<String, Fault> {
        self.run_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| Fault::new("app.not-ready", "记忆周目尚未初始化"))
    }
}

impl CompletedRounds for CycleRuntime {
    fn completed(&self, _: CompletedRound) {
        // 执行器只在 RoundEnded 确认并释放记录锁后通知；失败事件留待后续本地恢复。
        let _ = self.drain_cycle_events();
    }

    fn run_id(&self) -> Option<String> {
        self.run_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn prepare_cycle_event(&self, event: CycleLifecycleEvent) -> Result<(), Fault> {
        if event.session_id != self.session_id
            || event.run_id != self.run_id()?
            || event.catalog_revision != self.catalog.revision()
        {
            return Err(Fault::bad_request());
        }
        self.storage
            .with_memory_repository(|repository, connection| {
                repository
                    .prepare_cycle_event(connection, event)
                    .map(|_| ())
            })
    }

    fn drain_cycle_events(&self) -> Result<(), Fault> {
        let run_id = self.run_id()?;
        self.storage
            .with_memory(
                &self.session_id,
                &self.catalog,
                |repository, connection, port| {
                    let boundary = port.boundary()?;
                    let pending = repository.pending_cycle_events(
                        connection,
                        &run_id,
                        MAX_CYCLE_EVENT_PAGE,
                    )?;
                    for event in pending {
                        let discard = event.session_id != boundary.session_id
                            || event.history_revision != boundary.history_revision
                            || event.source_record_seq >= port.next_sequence()
                            || (event.source_record_seq != 0
                                && !port.has_current_record(event.source_record_seq));
                        if discard {
                            repository.delete_cycle_event(connection, &run_id, &event.event_id)?;
                            continue;
                        }
                        if event.catalog_revision != self.catalog.revision() {
                            return Err(aoidos_memory::error::Error::Rejected(
                                aoidos_memory::error::Reason::StaleVersion,
                            ));
                        }
                        let node = SceneNodeKey {
                            scene_id: &event.scene_id,
                            history_revision: event.history_revision,
                            entry_record_seq: event.entry_record_seq,
                        };
                        match event.kind {
                            CycleEventKind::SceneEntered => {
                                if !port
                                    .valid_scene_entry(&event.scene_id, event.source_record_seq)?
                                {
                                    return Err(aoidos_memory::error::Error::Rejected(
                                        aoidos_memory::error::Reason::InvalidSource,
                                    ));
                                }
                                repository.begin_scene_progress(
                                    connection,
                                    port,
                                    BeginSceneProgress {
                                        run_id: &run_id,
                                        node,
                                        catalog_revision: &event.catalog_revision,
                                        baseline: &event.evidence,
                                    },
                                )?;
                            }
                            CycleEventKind::RoundCompleted => {
                                let round_id = event.round_id.as_deref().ok_or(
                                    aoidos_memory::error::Error::Rejected(
                                        aoidos_memory::error::Reason::InvalidSource,
                                    ),
                                )?;
                                if !port.valid_completed_round(
                                    round_id,
                                    &event.scene_id,
                                    event.entry_record_seq,
                                    event.source_record_seq,
                                )? {
                                    return Err(aoidos_memory::error::Error::Rejected(
                                        aoidos_memory::error::Reason::InvalidSource,
                                    ));
                                }
                                repository.complete_scene_progress(
                                    connection,
                                    port,
                                    CompleteSceneProgress {
                                        run_id: &run_id,
                                        node,
                                        catalog_revision: &event.catalog_revision,
                                        round_id,
                                        completed_record_seq: event.source_record_seq,
                                        evidence: &event.evidence,
                                    },
                                )?;
                            }
                        }
                        repository.delete_cycle_event(connection, &run_id, &event.event_id)?;
                    }
                    Ok(())
                },
            )
            .map_err(crate::storage::memory_config_error)
    }

    fn ensure_current_node(
        &self,
        scene_id: &str,
        history_revision: u64,
        baseline: &[aoidos_memory::model::SceneRuleEvidence],
    ) -> Result<(), Fault> {
        let run_id = self.run_id()?;
        self.storage
            .with_memory(
                &self.session_id,
                &self.catalog,
                |repository, connection, port| {
                    let boundary = port.boundary()?;
                    if boundary.history_revision != history_revision
                        || boundary.session_id != self.session_id
                    {
                        return Err(aoidos_memory::error::Error::Rejected(
                            aoidos_memory::error::Reason::StaleVersion,
                        ));
                    }
                    let entry_record_seq =
                        port.scene_entry_seq_before(scene_id, port.next_sequence())?;
                    let node = SceneNodeKey {
                        scene_id,
                        history_revision,
                        entry_record_seq,
                    };
                    if repository
                        .scene_progress(connection, port, &run_id, node)?
                        .is_none()
                    {
                        repository.begin_scene_progress(
                            connection,
                            port,
                            BeginSceneProgress {
                                run_id: &run_id,
                                node,
                                catalog_revision: self.catalog.revision(),
                                baseline,
                            },
                        )?;
                    }
                    Ok(())
                },
            )
            .map_err(crate::storage::memory_config_error)
    }
}
