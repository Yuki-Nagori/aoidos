//! 记忆设置命令只编解码类型化 DTO；版本、继承和事务由记忆仓储负责。

use crate::{ipc::CmdError, store_commands::StorageService};
use aoidos_engine::storage::{
    AppConfig, Classification, ConfigSaved, MemoryEntryCursor, MemoryQuestion, PolicyOverrides,
    PolicyParameters, QuestionAnswer, QuestionCandidate, QuestionCursor, QuestionIdentity,
    QuestionPage, QuestionResolution, QuestionStatus, ScriptConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryPageArgs<T> {
    run_id: String,
    cursor: Option<T>,
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnswerQuestionArgs {
    run_id: String,
    resolution: QuestionResolutionDto,
}

/// Tauri 边界使用 camelCase；不改变仓储持久化的既有枚举字段名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum QuestionCandidateDto {
    Entity {
        #[serde(rename = "entityId")]
        entity_id: String,
        #[serde(
            default,
            rename = "displayName",
            skip_serializing_if = "Option::is_none"
        )]
        display_name: Option<String>,
    },
    MemoryClassification {
        classification: Classification,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum QuestionAnswerDto {
    SelectCandidate {
        candidate: QuestionCandidateDto,
    },
    ProposeEntity {
        #[serde(rename = "displayName")]
        display_name: String,
    },
    None,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuestionDraftDto {
    version: u32,
    identity: QuestionIdentity,
    history_revision: u64,
    entity_revision: u64,
    evidence_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mentioned_name: Option<String>,
    candidates: Vec<QuestionCandidateDto>,
}

/// 使用 IPC 字段命名约定的待确认问题响应，不用于持久化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryQuestionDto {
    question_id: String,
    run_id: String,
    question_key: Vec<u8>,
    revision: u64,
    draft: QuestionDraftDto,
    status: QuestionStatus,
    answer: Option<QuestionAnswerDto>,
}

/// 使用 IPC 字段命名约定的问题分页响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionPageDto {
    questions: Vec<MemoryQuestionDto>,
    next_cursor: Option<QuestionCursor>,
    history_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QuestionResolutionDto {
    question_id: String,
    expected_revision: u64,
    expected_history_revision: u64,
    answer: QuestionAnswerDto,
}

impl From<QuestionCandidateDto> for QuestionCandidate {
    fn from(value: QuestionCandidateDto) -> Self {
        match value {
            QuestionCandidateDto::Entity { entity_id, .. } => Self::Entity { entity_id },
            QuestionCandidateDto::MemoryClassification { classification } => {
                Self::MemoryClassification { classification }
            }
        }
    }
}

impl QuestionCandidateDto {
    fn from_model(value: QuestionCandidate, names: &BTreeMap<String, String>) -> Self {
        match value {
            QuestionCandidate::Entity { entity_id } => Self::Entity {
                display_name: names.get(&entity_id).cloned(),
                entity_id,
            },
            QuestionCandidate::MemoryClassification { classification } => {
                Self::MemoryClassification { classification }
            }
        }
    }
}

impl From<QuestionAnswerDto> for QuestionAnswer {
    fn from(value: QuestionAnswerDto) -> Self {
        match value {
            QuestionAnswerDto::SelectCandidate { candidate } => Self::SelectCandidate {
                candidate: candidate.into(),
            },
            QuestionAnswerDto::ProposeEntity { display_name } => {
                Self::ProposeEntity { display_name }
            }
            QuestionAnswerDto::None => Self::None,
            QuestionAnswerDto::Skip => Self::Skip,
        }
    }
}

impl QuestionAnswerDto {
    fn from_model(value: QuestionAnswer, names: &BTreeMap<String, String>) -> Self {
        match value {
            QuestionAnswer::SelectCandidate { candidate } => Self::SelectCandidate {
                candidate: QuestionCandidateDto::from_model(candidate, names),
            },
            QuestionAnswer::ProposeEntity { display_name } => Self::ProposeEntity { display_name },
            QuestionAnswer::None => Self::None,
            QuestionAnswer::Skip => Self::Skip,
        }
    }
}

impl MemoryQuestionDto {
    fn from_model(question: MemoryQuestion, names: &BTreeMap<String, String>) -> Self {
        Self {
            question_id: question.question_id,
            run_id: question.run_id,
            question_key: question.question_key.to_vec(),
            revision: question.revision,
            draft: QuestionDraftDto {
                version: question.draft.version,
                identity: question.draft.identity,
                history_revision: question.draft.history_revision,
                entity_revision: question.draft.entity_revision,
                evidence_revision: question.draft.evidence_revision,
                mentioned_name: question.draft.mentioned_name,
                candidates: question
                    .draft
                    .candidates
                    .into_iter()
                    .map(|candidate| QuestionCandidateDto::from_model(candidate, names))
                    .collect(),
            },
            status: question.status,
            answer: question
                .answer
                .map(|answer| QuestionAnswerDto::from_model(answer, names)),
        }
    }
}

impl QuestionPageDto {
    fn from_page(page: QuestionPage, names: &BTreeMap<String, String>) -> Self {
        Self {
            questions: page
                .questions
                .into_iter()
                .map(|question| MemoryQuestionDto::from_model(question, names))
                .collect(),
            next_cursor: page.next_cursor,
            history_revision: page.history_revision,
        }
    }
}

impl From<QuestionResolutionDto> for QuestionResolution {
    fn from(resolution: QuestionResolutionDto) -> Self {
        Self {
            question_id: resolution.question_id,
            expected_revision: resolution.expected_revision,
            expected_history_revision: resolution.expected_history_revision,
            answer: resolution.answer.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntryArgs {
    run_id: String,
    entry_id: String,
    expected_version_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchStatusArgs {
    run_id: String,
    batch_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectMaterialsArgs {
    run_id: String,
    cursor: Option<aoidos_engine::storage::MemoryMaterialCursor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveAppConfig {
    expected_version: u64,
    parameters: PolicyParameters,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScriptConfigArgs {
    script_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveScriptConfig {
    script_id: String,
    expected_version: u64,
    expected_app_version: u64,
    overrides: PolicyOverrides,
}

fn decode<T: serde::de::DeserializeOwned>(
    request: &tauri::ipc::Request<'_>,
) -> Result<T, CmdError> {
    crate::ipc::decode_request(request, "记忆设置参数不合法")
}

/// 返回应用当前已确认参数；首次读取返回受版本管理的开发初值。
/// # Errors
/// 存储不可用或配置损坏时返回稳定的记忆 / 存储错误。
#[tauri::command]
pub async fn memory_get_app_config(
    state: tauri::State<'_, StorageService>,
) -> Result<AppConfig, CmdError> {
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_app_config()).await
}

/// 以应用版本 CAS 保存完整默认参数，不重算已存在周目的冻结策略。
/// # Errors
/// 参数非法或版本过期返回 app.bad-request，存储故障保留 store.*。
#[tauri::command]
pub async fn memory_save_app_config(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<ConfigSaved, CmdError> {
    let SaveAppConfig {
        expected_version,
        parameters,
    } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.save_memory_app_config(expected_version, parameters)).await
}

/// 读取剧本显式覆盖与继承后的完整配置。
/// # Errors
/// 非法剧本 ID 返回 app.bad-request，损坏配置不会回退到默认值。
#[tauri::command]
pub async fn memory_get_script_config(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<ScriptConfig, CmdError> {
    let ScriptConfigArgs { script_id } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_script_config(&script_id)).await
}

/// 同时比对剧本覆盖版本与应用默认版本，保存前再次校验合并策略。
/// # Errors
/// 任一版本已变化或整组策略不合法时拒绝覆盖草稿。
#[tauri::command]
pub async fn memory_save_script_config(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<ConfigSaved, CmdError> {
    let SaveScriptConfig {
        script_id,
        expected_version,
        expected_app_version,
        overrides,
    } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || {
        storage.save_memory_script_config(
            &script_id,
            expected_version,
            expected_app_version,
            overrides,
        )
    })
    .await
}

/// 按 SQLite 快照游标读取待确认问题；新问题留给下一次刷新。
/// # Errors
/// 历史修订变化、来源失效或页大小越界时拒绝续读。
#[tauri::command]
pub async fn memory_list_questions(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<QuestionPageDto, CmdError> {
    let MemoryPageArgs {
        run_id,
        cursor,
        limit,
    } = decode::<MemoryPageArgs<QuestionCursor>>(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_questions(&run_id, cursor, limit))
        .await
        .map(|(page, names)| QuestionPageDto::from_page(page, &names))
}

/// 以问题 revision 与 historyRevision 做原子回答。
/// # Errors
/// 迟到回答或已变化的来源 / 候选返回 app.bad-request。
#[tauri::command]
pub async fn memory_resolve_question(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<MemoryQuestionDto, CmdError> {
    let AnswerQuestionArgs { run_id, resolution } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.resolve_memory_question(&run_id, resolution.into()))
        .await
        .map(|(question, names)| MemoryQuestionDto::from_model(question, &names))
}

/// 按历史修订绑定游标有界读取记忆条目索引。
/// # Errors
/// 旧历史游标和非法页大小返回稳定的应用错误。
#[tauri::command]
pub async fn memory_list_entries(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::storage::MemoryEntryPage, CmdError> {
    let MemoryPageArgs {
        run_id,
        cursor,
        limit,
    } = decode::<MemoryPageArgs<MemoryEntryCursor>>(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_entries(&run_id, cursor, limit)).await
}

/// 读取当前仍有效的不可变记忆版本。
/// # Errors
/// 版本指针变化或正文 / 来源无法验证时拒绝返回内容。
#[tauri::command]
pub async fn memory_get_entry(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::storage::MemoryEntryDetails, CmdError> {
    let EntryArgs {
        run_id,
        entry_id,
        expected_version_id,
    } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_entry(&run_id, &entry_id, &expected_version_id)).await
}

/// 主动读取已持久化批次状态；只返回完成摘要，不返回模型原始响应。
/// # Errors
/// 周目不匹配、未知批次或回执损坏时返回脱敏状态错误。
#[tauri::command]
pub async fn memory_get_batch_status(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::storage::MemoryBatchStatus, CmdError> {
    let BatchStatusArgs { run_id, batch_id } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.memory_batch_status(&run_id, &batch_id)).await
}

/// 本地扫描并登记当前因果分支的有效素材；此命令不冻结批次、不访问模型。
/// # Errors
/// 周目游标过期、记录待恢复或素材 / SQLite 校验失败时返回稳定错误。
#[tauri::command]
pub async fn memory_collect_materials(
    state: tauri::State<'_, StorageService>,
    request: tauri::ipc::Request<'_>,
) -> Result<aoidos_engine::storage::MemoryMaterialPage, CmdError> {
    let CollectMaterialsArgs { run_id, cursor } = decode(&request)?;
    state.ready()?;
    let storage = state.storage.clone();
    memory_job(move || storage.collect_memory_materials(&run_id, cursor)).await
}

async fn memory_job<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, aoidos_engine::fault::Fault> + Send + 'static,
) -> Result<T, CmdError> {
    aoidos_engine::blocking::run(job)
        .await
        .map_err(CmdError::from)
}

#[cfg(test)]
mod tests;
