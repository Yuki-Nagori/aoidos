use crate::ipc::CmdError;
use aoidos_llm::config::{
    LlmProfile, MAX_PROFILES, ProfileError, ProfileStore, validate_identifier,
};
use aoidos_llm::credentials::{CredentialStore, CredentialVault, KeyStatus};
use aoidos_llm::platform::NativePrompt;
use aoidos_store::error::StoreError;
use secrecy::SecretString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// llm 数据目录（app-data/llm/）：profile 与凭据降级文件的根。
static LLM_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 逻辑读改写的进程内串行锁（storage.md「并发与锁」）：原子写只保证单次
/// 覆写的物理完整，读-改-写周期由这两把锁串行；命令层持有，业务 crate 不感知。
/// 原生输入等待用户时不持锁，避免长时间阻塞另一把锁的使用方。
static PROFILE_LOCK: Mutex<()> = Mutex::new(());
static KEY_LOCK: Mutex<()> = Mutex::new(());
static PROMPT_ACTIVE: AtomicBool = AtomicBool::new(false);

struct PromptPermit<'a>(&'a AtomicBool);
impl<'a> PromptPermit<'a> {
    fn acquire(active: &'a AtomicBool) -> Result<Self, CmdError> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(prompt_busy)?;
        Ok(Self(active))
    }
}
impl Drop for PromptPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn prompt_busy(_: bool) -> CmdError {
    CmdError::new("app.busy", "原生密钥输入已打开", None)
}

fn profile_lock() -> MutexGuard<'static, ()> {
    // 锁中毒说明上次持锁期间 panic；文件操作是原子的，状态仍有效，忽略中毒。
    PROFILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key_lock() -> MutexGuard<'static, ()> {
    KEY_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 由桌面装配或独立集成进程注入 llm 数据目录；重复注入忽略。
pub fn init_llm_dir(dir: PathBuf) {
    let _ = LLM_DIR.set(dir);
}

fn llm_dir() -> Result<&'static PathBuf, CmdError> {
    LLM_DIR.get().ok_or_else(not_ready)
}

fn not_ready() -> CmdError {
    CmdError::new("app.not-ready", "LLM 配置尚未初始化", None)
}

/// 删除确认载荷。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deleted {
    pub deleted: bool,
}

fn profile_store() -> Result<ProfileStore, CmdError> {
    Ok(ProfileStore::new(llm_dir()?))
}

fn credential_vault() -> Result<CredentialVault, CmdError> {
    // vault 构造不落盘；目录缺失时首次写入会建。
    Ok(CredentialVault::new(llm_dir()?))
}

/// 读取全部 profile；目录缺失视为空列表。
///
/// # Errors
/// 目录不可读或文件损坏（store.corrupt）时返回。
#[tauri::command]
pub fn llm_list_profiles() -> Result<LlmProfileList, CmdError> {
    list_profiles_in(&profile_store()?)
}

fn list_profiles_in(store: &ProfileStore) -> Result<LlmProfileList, CmdError> {
    Ok(LlmProfileList {
        items: store.load().map_err(CmdError::from)?,
    })
}

/// 保存单个 profile（存在同 id 即覆盖，不存在追加）。校验失败、数量超上限
/// 为 app.bad-request。
///
/// # Errors
/// 校验失败、目录不可写时返回。
#[tauri::command]
pub fn llm_save_profile(profile: LlmProfile) -> Result<LlmProfile, CmdError> {
    let _guard = profile_lock();
    save_profile_in(&profile_store()?, profile)
}

fn save_profile_in(store: &ProfileStore, profile: LlmProfile) -> Result<LlmProfile, CmdError> {
    profile.validate().map_err(bad_profile)?;
    let mut profiles = store.load().map_err(CmdError::from)?;
    if let Some(idx) = profiles
        .iter()
        .position(|p| p.profile_id == profile.profile_id)
    {
        profiles[idx] = profile.clone()
    } else {
        if profiles.len() >= MAX_PROFILES {
            return Err(CmdError::new(
                "app.bad-request",
                format!("profile 数量已达上限 {MAX_PROFILES}"),
                None,
            ));
        }
        profiles.push(profile.clone());
    }
    store.save(&profiles).map_err(CmdError::from)?;
    Ok(profile)
}

/// profile 结构校验失败 → app.bad-request（message 面向用户，code 供分支）。
fn bad_profile(error: ProfileError) -> CmdError {
    CmdError::new("app.bad-request", error.to_string(), None)
}

/// 提交前一次读取配置及凭据；返回不可变副本，后续重试不重新查 key。
///
/// # Errors
/// 未知 profile 为 app.not-found；存储 / 凭据不可读按 store.* 透传。
pub(crate) fn freeze_submission(
    profile_id: &str,
) -> Result<
    (
        aoidos_llm::config::FrozenProfile,
        Option<aoidos_llm::proxy::ProxyAuth>,
    ),
    CmdError,
> {
    validate_identifier("profileId", profile_id).map_err(bad_profile)?;
    let profile = {
        let _guard = profile_lock();
        profile_store()?
            .load()
            .map_err(CmdError::from)?
            .into_iter()
            .find(|profile| profile.profile_id == profile_id)
            .ok_or_else(profile_missing)?
    };
    let _guard = key_lock();
    let credentials = credential_vault()?;
    let frozen = aoidos_llm::config::freeze(&profile, &credentials).map_err(CmdError::from)?;
    let auth_ref = match &profile.proxy {
        aoidos_llm::config::ProxyConfig::Manual { auth_ref, .. } => auth_ref.as_deref(),
        _ => None,
    };
    let auth =
        aoidos_llm::proxy::resolve_proxy_auth(&credentials, auth_ref).map_err(CmdError::from)?;
    Ok((frozen, auth))
}
fn profile_missing() -> CmdError {
    CmdError::new("app.not-found", "配置不存在", None)
}

/// 删除 profile；不存在时也返回成功（幂等）。
///
/// # Errors
/// 持久化失败时返回。
#[tauri::command]
pub fn llm_delete_profile(profile_id: String) -> Result<Deleted, CmdError> {
    let _guard = profile_lock();
    delete_profile_in(&profile_store()?, &profile_id)
}

fn delete_profile_in(store: &ProfileStore, profile_id: &str) -> Result<Deleted, CmdError> {
    let mut profiles = store.load().map_err(CmdError::from)?;
    let before = profiles.len();
    profiles.retain(|p| p.profile_id != profile_id);
    if profiles.len() != before {
        store.save(&profiles).map_err(CmdError::from)?;
    }
    Ok(Deleted { deleted: true })
}

/// Rust 侧读取凭据并派生状态；IPC 只返回 set + hint。
///
/// # Errors
/// 凭据后端不可读时返回 store.*。
#[tauri::command]
pub fn llm_get_key_status(provider_id: String) -> Result<KeyStatus, CmdError> {
    validate_identifier("providerId", &provider_id).map_err(bad_profile)?;
    let _guard = key_lock();
    key_status_with(&provider_id, &credential_vault()?)
}

fn key_status_with(provider_id: &str, store: &dyn CredentialStore) -> Result<KeyStatus, CmdError> {
    validate_identifier("providerId", provider_id).map_err(bad_profile)?;
    store.status(provider_id).map_err(CmdError::from)
}

/// UI 主线程调度器；Sync 使命令 Future 可由 Tauri 安全调度。
pub type NativeDispatch = dyn Fn(Box<dyn FnOnce() + Send>) -> tauri::Result<()> + Sync;

/// 同型密钥操作，原生等待期间不持业务锁。
///
/// # Errors
/// 无效操作、重复原生输入、调度或凭据服务失败按统一命令错误返回。
pub async fn set_key_with_dispatch(
    provider_id: String,
    action: String,
    prompt: NativePrompt,
    dispatch: &NativeDispatch,
) -> Result<KeyStatus, CmdError> {
    validate_identifier("providerId", &provider_id).map_err(bad_profile)?;
    set_key_in(
        &provider_id,
        &action,
        &credential_vault()?,
        prompt,
        dispatch,
    )
    .await
}

async fn set_key_in(
    provider_id: &str,
    action: &str,
    store: &dyn CredentialStore,
    prompt: NativePrompt,
    dispatch: &NativeDispatch,
) -> Result<KeyStatus, CmdError> {
    validate_identifier("providerId", provider_id).map_err(bad_profile)?;
    match action {
        "clear" => return clear_key_in(provider_id, store),
        "set" => {}
        _ => return Err(CmdError::new("app.bad-request", "未知凭据操作", None)),
    }
    let permit = PromptPermit::acquire(&PROMPT_ACTIVE)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let label = format!("provider {provider_id}");
    dispatch(Box::new(move || {
        let result = prompt(&label);
        drop(permit);
        let _ = tx.send(result);
    }))
    .map_err(native_dispatch_error)?;
    let result = rx.await.map_err(native_receive_error)?;
    save_prompt_result(provider_id, store, result)
}

fn clear_key_in(provider_id: &str, store: &dyn CredentialStore) -> Result<KeyStatus, CmdError> {
    let _guard = key_lock();
    store.clear_key(provider_id).map_err(CmdError::from)?;
    key_status_with(provider_id, store)
}

fn save_prompt_result(
    provider_id: &str,
    store: &dyn CredentialStore,
    result: Result<Option<String>, StoreError>,
) -> Result<KeyStatus, CmdError> {
    let _guard = key_lock();
    let secret = match result {
        Ok(Some(secret)) => secret,
        // 用户取消：保留旧值，返回当前状态。
        Ok(None) => return key_status_with(provider_id, store),
        Err(e) => return Err(CmdError::from(e)),
    };
    store
        .set_key(provider_id, SecretString::from(secret))
        .map_err(CmdError::from)?;
    key_status_with(provider_id, store)
}

/// profile 列表载荷（无分页：硬上限 50 个，超出校验拒绝）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProfileList {
    pub items: Vec<LlmProfile>,
}

fn native_dispatch_error(_: tauri::Error) -> CmdError {
    CmdError::new("store.io", "原生输入无法调度", None)
}
fn native_receive_error(_: tokio::sync::oneshot::error::RecvError) -> CmdError {
    CmdError::new("store.io", "原生输入未返回", None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_llm::platform::platform_prompt;
    use aoidos_store::error::Result;
    use secrecy::ExposeSecret;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aoidos-cmd-llm-{}-{}",
            tag,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 进程内唯一 id：命令级测试经共享的 `OnceLock` 目录运行，断言只能相对化。
    fn unique(tag: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{tag}-{nanos}")
    }

    /// 测试内存凭据库：注入确定性行为，不触真实 OS 凭据服务。
    #[derive(Default)]
    struct MemoryKeys(Mutex<BTreeMap<String, String>>);

    impl CredentialStore for MemoryKeys {
        fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(provider_id.to_owned(), secret.expose_secret().to_owned());
            Ok(())
        }

        fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(provider_id)
                .map(|raw| SecretString::from(raw.clone())))
        }

        fn clear_key(&self, provider_id: &str) -> Result<()> {
            self.0.lock().unwrap().remove(provider_id);
            Ok(())
        }
    }

    fn deepseek_profile(profile_id: &str) -> LlmProfile {
        LlmProfile {
            profile_id: profile_id.into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            mode: aoidos_llm::config::ProfileMode::Completion,
            thinking: false,
            sampling: aoidos_llm::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 2048,
            },
            proxy: Default::default(),
        }
    }

    fn rejecting_dispatch(_: Box<dyn FnOnce() + Send>) -> tauri::Result<()> {
        Err(tauri::Error::Io(std::io::Error::other("shutdown")))
    }

    #[tokio::test]
    async fn native_dispatch_confirms_cancels_and_reports_shutdown() {
        let keys = MemoryKeys::default();
        let direct = |job: Box<dyn FnOnce() + Send>| {
            job();
            Ok(())
        };
        let status = set_key_in("p", "set", &keys, fake_prompt, &direct)
            .await
            .unwrap();
        assert!(status.set);
        // 原生对话框未返回时，重复命令不得调度第二个对话框或改写旧凭据。
        let pending_job = std::sync::Arc::new(Mutex::new(None));
        let dispatch_job = pending_job.clone();
        let deferred = move |job: Box<dyn FnOnce() + Send>| {
            *dispatch_job.lock().unwrap() = Some(job);
            Ok(())
        };
        let mut first = std::pin::pin!(set_key_in("p", "set", &keys, fake_prompt, &deferred));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(first.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(
            set_key_in("p", "set", &keys, fake_prompt, &rejecting_dispatch)
                .await
                .unwrap_err()
                .code(),
            "app.busy"
        );
        assert_eq!(key_status_with("p", &keys).unwrap(), status);
        pending_job.lock().unwrap().take().unwrap()();
        assert_eq!(first.await.unwrap(), status);
        let cancelled = set_key_in("p", "set", &keys, |_| Ok(None), &direct)
            .await
            .unwrap();
        assert_eq!(status, cancelled);
        assert_eq!(
            set_key_in("p", "set", &keys, fake_prompt, &rejecting_dispatch)
                .await
                .unwrap_err()
                .code(),
            "store.io"
        );
        assert_eq!(
            set_key_in("p", "set", &keys, fake_prompt, &|_| Ok(()))
                .await
                .unwrap_err()
                .code(),
            "store.io"
        );
        assert!(
            !set_key_in("p", "clear", &keys, fake_prompt, &direct)
                .await
                .unwrap()
                .set
        );
        let status = set_key_in(
            "deepseek",
            "set",
            &keys,
            |label| {
                assert!(label.contains("deepseek"));
                Ok(Some("sk-new-1234".into()))
            },
            &direct,
        )
        .await
        .unwrap();
        assert_eq!(
            aoidos_json::to_value(status).unwrap(),
            serde_json::json!({"set": true, "hint": "1234"})
        );
        assert_eq!(
            keys.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-new-1234"
        );
        let err = set_key_in(
            "deepseek",
            "set",
            &keys,
            |_| {
                Err(StoreError::from_io(std::io::Error::other(
                    "credential ui failed",
                )))
            },
            &direct,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "store.io");
        assert!(
            !aoidos_json::to_string(&err)
                .unwrap()
                .contains("sk-new-1234")
        );
        // 非设置操作及非法标识符在调度前退出，不能弹窗或修改凭据。
        let no_dispatch = rejecting_dispatch;
        for id in ["", " ", "bad\nname", &"x".repeat(257)] {
            for action in ["set", "clear"] {
                assert_eq!(
                    set_key_in(id, action, &keys, fake_prompt, &no_dispatch)
                        .await
                        .unwrap_err()
                        .code(),
                    "app.bad-request"
                );
            }
            assert_eq!(
                key_status_with(id, &keys).unwrap_err().code(),
                "app.bad-request"
            );
        }
        assert!(keys.0.lock().unwrap().keys().all(|id| id == "deepseek"));
        assert_eq!(
            set_key_in("p", "bogus", &keys, fake_prompt, &no_dispatch)
                .await
                .unwrap_err()
                .code(),
            "app.bad-request"
        );
        assert!(
            !set_key_in("deepseek", "clear", &keys, fake_prompt, &no_dispatch)
                .await
                .unwrap()
                .set
        );
        init_llm_dir(tdir("async-command"));
        let provider = unique("async-probe");
        // 未知操作必须在原生交互前拒绝，装配入口也覆盖。
        assert_eq!(
            set_key_with_dispatch(provider, "bogus".into(), platform_prompt(), &direct)
                .await
                .unwrap_err()
                .code(),
            "app.bad-request"
        );
    }

    #[test]
    fn prompt_permit_rejects_duplicates_and_releases_on_drop() {
        let active = AtomicBool::new(false);
        let permit = PromptPermit::acquire(&active).unwrap();
        assert_eq!(
            PromptPermit::acquire(&active).err().unwrap().code(),
            "app.busy"
        );
        drop(permit);
        assert!(PromptPermit::acquire(&active).is_ok());
    }

    #[test]
    fn not_ready_maps_to_dedicated_code() {
        assert_eq!(not_ready().code(), "app.not-ready");
    }

    #[test]
    fn profile_round_trip_overwrite_and_idempotent_delete() {
        let dir = tdir("profile-crud");
        let store = ProfileStore::new(&dir);
        let profile = deepseek_profile("narration");
        let saved = save_profile_in(&store, profile.clone()).unwrap();
        assert_eq!(saved, profile);
        assert_eq!(
            list_profiles_in(&store).unwrap().items,
            vec![profile.clone()]
        );
        // 同 id 覆盖不追加。
        let mut updated = profile.clone();
        updated.model = "deepseek-flash".into();
        save_profile_in(&store, updated).unwrap();
        let items = list_profiles_in(&store).unwrap().items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].model, "deepseek-flash");
        // 幂等删除。
        assert!(delete_profile_in(&store, "narration").unwrap().deleted);
        assert!(delete_profile_in(&store, "narration").unwrap().deleted);
        assert!(list_profiles_in(&store).unwrap().items.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn profile_count_is_capped_at_fifty() {
        let dir = tdir("profile-cap");
        let store = ProfileStore::new(&dir);
        let profiles: Vec<LlmProfile> = (0..MAX_PROFILES)
            .map(|i| deepseek_profile(&format!("p{i}")))
            .collect();
        store.save(&profiles).unwrap();
        let err = save_profile_in(&store, deepseek_profile("overflow")).unwrap_err();
        assert_eq!(err.code(), "app.bad-request");
        // 同 id 覆盖不受上限影响。
        save_profile_in(&store, deepseek_profile("p0")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_profile_maps_to_bad_request() {
        let dir = tdir("profile-invalid");
        let store = ProfileStore::new(&dir);
        let mut profile = deepseek_profile("x");
        profile.model = "  ".into();
        let err = save_profile_in(&store, profile.clone()).unwrap_err();
        assert_eq!(err.code(), "app.bad-request");
        profile.model = "m".into();
        profile.proxy = aoidos_llm::config::ProxyConfig::Manual {
            url: "file://x".into(),
            auth_ref: None,
        };
        assert!(save_profile_in(&store, profile).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn commands_round_trip_through_shared_dir() {
        // 命令级冒烟：OnceLock 目录全进程共享（先到先得），断言只用唯一 id 相对化。
        let dir = tdir("commands");
        init_llm_dir(dir);
        let profile = deepseek_profile(&unique("rt"));
        llm_save_profile(profile.clone()).unwrap();
        let items = llm_list_profiles().unwrap().items;
        assert!(items.iter().any(|p| p.profile_id == profile.profile_id));
        llm_delete_profile(profile.profile_id.clone()).unwrap();
        let items = llm_list_profiles().unwrap().items;
        assert!(!items.iter().any(|p| p.profile_id == profile.profile_id));
    }

    #[test]
    fn key_status_reports_set_and_hint_without_plaintext() {
        let keys = MemoryKeys::default();
        keys.set_key("deepseek", SecretString::from(String::from("sk-abcdef99")))
            .unwrap();
        let status = key_status_with("deepseek", &keys).unwrap();
        assert_eq!(
            aoidos_json::to_value(&status).unwrap(),
            serde_json::json!({ "set": true, "hint": "ef99" })
        );
        assert!(!key_status_with("missing", &keys).unwrap().set);
    }

    /// clear 路径绝不触发输入；传入的 Ready 注入器若被误用会替换出可见的
    /// 新值，由断言暴露（函数体同时被 set 用例复用覆盖）。
    fn fake_prompt(label: &str) -> Result<Option<String>> {
        Ok(Some(format!("sk-prompted-{label}")))
    }

    fn clear_action(store: &dyn CredentialStore) -> std::result::Result<KeyStatus, CmdError> {
        clear_key_in("deepseek", store)
    }

    #[test]
    fn clear_removes_key_and_stays_idempotent() {
        let keys = MemoryKeys::default();
        keys.set_key("deepseek", SecretString::from(String::from("sk-temp")))
            .unwrap();
        let status = clear_action(&keys).unwrap();
        assert!(!status.set);
        // 未设置时再清：仍成功。
        let status = clear_action(&keys).unwrap();
        assert_eq!(
            aoidos_json::to_value(status).unwrap(),
            serde_json::json!({ "set": false, "hint": null })
        );
    }

    #[test]
    fn status_command_reads_through_shared_vault() {
        // 命令级：对从未设置过的唯一 id 读状态 / 清除（NoEntry 幂等路径），
        // 不写入任何真实凭据。目录由先到的测试注入（OnceLock 全进程共享）。
        init_llm_dir(tdir("vault-probe"));
        let provider = unique("vault-probe");
        // 命令级检查目录注入与删除墓碑；OS 无会话时删除报错，墓碑仍阻止旧值读取。
        // 服务真实读写清由三平台 test:native 留证，错误语义另有注入直测。
        let _ = clear_key_in(&provider, &credential_vault().unwrap());
        assert!(!llm_get_key_status(provider).unwrap().set);
    }
    #[tokio::test]
    async fn freeze_and_debug_submission_use_saved_configuration_once() {
        init_llm_dir(tdir("turn-profile"));
        let id = unique("turn-profile");
        let mut profile = deepseek_profile(&id);
        profile.model = "deepseek-v4-pro".into();
        profile.proxy = aoidos_llm::config::ProxyConfig::Manual {
            url: "http://127.0.0.1:3128".into(),
            auth_ref: Some("turn-proxy-auth".into()),
        };
        llm_save_profile(profile).unwrap();
        {
            let _guard = key_lock();
            let file = aoidos_llm::credentials::CredentialFile::new(llm_dir().unwrap());
            file.set_key("deepseek", SecretString::from("local-fixture".to_owned()))
                .unwrap();
            file.set_key(
                "turn-proxy-auth",
                SecretString::from("user:proxy-fixture".to_owned()),
            )
            .unwrap();
        }
        use aoidos_engine::game::product::ProfileSource;
        let resolved = crate::game_commands::SavedProfiles.freeze(&id).unwrap();
        assert_eq!(
            resolved.frozen.credential.unwrap().expose_secret(),
            "local-fixture"
        );
        assert!(resolved.proxy_auth.is_some());
        for proxy in [
            aoidos_llm::config::ProxyConfig::None,
            aoidos_llm::config::ProxyConfig::System,
            aoidos_llm::config::ProxyConfig::Manual {
                url: "http://127.0.0.1:3128".into(),
                auth_ref: None,
            },
        ] {
            let id = unique("turn-without-proxy-auth");
            let mut profile = deepseek_profile(&id);
            profile.proxy = proxy;
            llm_save_profile(profile).unwrap();
            let (_, auth) = freeze_submission(&id).unwrap();
            assert!(auth.is_none());
        }
        assert_eq!(
            freeze_submission(&unique("missing")).err().unwrap().code(),
            "app.not-found"
        );
        assert_eq!(
            freeze_submission("").err().unwrap().code(),
            "app.bad-request"
        );
        let service = crate::turn_commands::TurnService::new(
            std::sync::Arc::new(crate::turn_commands::WindowEvents::new(|_, _| Ok(()))),
            aoidos_llm::proxy::SystemProxySnapshot::default(),
        )
        .unwrap();
        let input = aoidos_llm::provider::ProviderInput::Completion(
            aoidos_llm::provider::CompletionInput {
                prompt: "local".into(),
            },
        );
        assert_eq!(
            crate::turn_commands::submit(&service, id.clone(), input.clone(), "unknown".into())
                .unwrap_err()
                .code(),
            "app.not-found"
        );
        let accepted = crate::turn_commands::submit(
            &service,
            id,
            input,
            aoidos_engine::fixture::GUARD_SPEC_ID.into(),
        )
        .unwrap();
        service.coordinator.cancel(&accepted.turn_id).await.unwrap();
    }
}
