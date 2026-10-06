use crate::ipc::CmdError;
use mythos_llm::config::{LlmProfile, ProfileError, ProfileStore};
use mythos_llm::credentials::{CredentialStore, CredentialVault, KeyStatus};
use mythos_llm::platform::{NativePrompt, platform_prompt};
use mythos_store::error::StoreError;
use secrecy::SecretString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// llm 数据目录（app-data/llm/）：profile 与凭据降级文件的根。
static LLM_DIR: OnceLock<PathBuf> = OnceLock::new();

/// profile 列表硬上限（契约：写明硬上限不超过 50 的列表可不分页）。
const MAX_PROFILES: usize = 50;

/// 逻辑读改写的进程内串行锁（storage.md「并发与锁」）：原子写只保证单次
/// 覆写的物理完整，读-改-写周期由这两把锁串行；命令层持有，业务 crate 不感知。
/// 原生输入等待用户时不持锁，避免长时间阻塞另一把锁的使用方。
static PROFILE_LOCK: Mutex<()> = Mutex::new(());
static KEY_LOCK: Mutex<()> = Mutex::new(());

fn profile_lock() -> MutexGuard<'static, ()> {
    // 锁中毒说明上次持锁期间 panic；文件操作是原子的，状态仍有效，忽略中毒。
    PROFILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn key_lock() -> MutexGuard<'static, ()> {
    KEY_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// setup 注入 llm 数据目录；重复注入忽略。
pub(crate) fn init_llm_dir(dir: PathBuf) {
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

/// 查询凭据状态（不读取密钥明文，只返回 set + hint）。
///
/// # Errors
/// 凭据后端不可读时返回 store.*。
#[tauri::command]
pub fn llm_get_key_status(provider_id: String) -> Result<KeyStatus, CmdError> {
    key_status_with(&provider_id, &credential_vault()?)
}

fn key_status_with(provider_id: &str, store: &dyn CredentialStore) -> Result<KeyStatus, CmdError> {
    store.status(provider_id).map_err(CmdError::from)
}

/// 设置 / 清除密钥。action="set" 时发起 Rust 原生输入（平台分发见
/// [`NativePrompt`]，未验证平台显式拒绝）；"clear" 直接删除。取消输入
/// 保留旧值，返回当前状态。
///
/// # Errors
/// 未知 action 或平台未验证原生输入为 app.bad-request；凭据后端失败为
/// store.*。
#[tauri::command]
pub fn llm_set_key(provider_id: String, action: String) -> Result<KeyStatus, CmdError> {
    llm_set_key_with(
        &provider_id,
        &action,
        &credential_vault()?,
        platform_prompt(),
    )
}

fn llm_set_key_with(
    provider_id: &str,
    action: &str,
    store: &dyn CredentialStore,
    prompt: NativePrompt,
) -> Result<KeyStatus, CmdError> {
    match action {
        "clear" => {
            let _guard = key_lock();
            store.clear_key(provider_id).map_err(CmdError::from)?;
            key_status_with(provider_id, store)
        }
        "set" => match prompt {
            NativePrompt::Ready(prompt_fn) => set_via_prompt(provider_id, store, prompt_fn),
            // 平台未验证原生输入：显式拒绝，不降级为 Webview 明文提交。
            NativePrompt::Unverified => Err(CmdError::new(
                "app.bad-request",
                "该平台的原生密钥输入尚未验证；不支持经 Webview 明文提交密钥",
                None,
            )),
        },
        other => Err(CmdError::new(
            "app.bad-request",
            format!("未知 action：{other}"),
            None,
        )),
    }
}

/// Ready 平台的设置流程：原生输入等待用户（此时不持锁，避免长时间阻塞
/// 并发的凭据命令），拿到密钥后才持 key 锁串行化写入；用户取消保留旧值。
fn set_via_prompt(
    provider_id: &str,
    store: &dyn CredentialStore,
    prompt_fn: fn(&str) -> Result<Option<String>, StoreError>,
) -> Result<KeyStatus, CmdError> {
    let label = format!("provider {provider_id}");
    let secret = match prompt_fn(&label) {
        Ok(Some(secret)) => secret,
        // 用户取消：保留旧值，返回当前状态。
        Ok(None) => return key_status_with(provider_id, store),
        Err(e) => return Err(CmdError::from(e)),
    };
    let _guard = key_lock();
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

#[cfg(test)]
mod tests {
    use super::*;
    use mythos_store::error::Result;
    use secrecy::ExposeSecret;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mythos-cmd-llm-{}-{}",
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
            mode: mythos_llm::config::ProfileMode::Completion,
            thinking: false,
            sampling: mythos_llm::sampling::Sampling {
                temperature: 1.0,
                max_tokens: 2048,
            },
            proxy: Default::default(),
        }
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
        profile.proxy = mythos_llm::config::ProxyConfig::Manual {
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
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({ "set": true, "hint": "ef99" })
        );
        assert!(!key_status_with("missing", &keys).unwrap().set);
    }

    /// clear 路径绝不触发输入；传入的 Ready 注入器若被误用会替换出可见的
    /// 新值，由断言暴露（函数体同时被 set 用例复用覆盖）。
    #[cfg(windows)]
    fn fake_prompt(label: &str) -> Result<Option<String>> {
        Ok(Some(format!("sk-prompted-{label}")))
    }

    #[cfg(windows)]
    fn clear_action(store: &dyn CredentialStore) -> std::result::Result<KeyStatus, CmdError> {
        llm_set_key_with("deepseek", "clear", store, NativePrompt::Ready(fake_prompt))
    }

    #[cfg(not(windows))]
    fn clear_action(store: &dyn CredentialStore) -> std::result::Result<KeyStatus, CmdError> {
        // clear 不经输入器；Unverified 占位即可。
        llm_set_key_with("deepseek", "clear", store, NativePrompt::Unverified)
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
            serde_json::to_value(status).unwrap(),
            serde_json::json!({ "set": false, "hint": null })
        );
    }

    #[cfg(windows)]
    #[test]
    fn set_via_native_prompt_replaces_and_cancel_keeps_old_value() {
        let keys = MemoryKeys::default();
        keys.set_key("deepseek", SecretString::from(String::from("sk-old-9999")))
            .unwrap();
        // 用户取消：保留旧值。
        let status = llm_set_key_with(
            "deepseek",
            "set",
            &keys,
            NativePrompt::Ready(|_label| Ok(None)),
        )
        .unwrap();
        assert_eq!(status.hint.as_deref(), Some("9999"));
        // 确认输入：提示语含 provider 标识，替换并返回新状态。
        let status = llm_set_key_with(
            "deepseek",
            "set",
            &keys,
            NativePrompt::Ready(|label| {
                assert!(label.contains("deepseek"));
                Ok(Some("sk-new-1234".into()))
            }),
        )
        .unwrap();
        assert_eq!(status.hint.as_deref(), Some("1234"));
        assert_eq!(
            keys.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-new-1234"
        );
        // 函数项注入器同样可用（clear 路径复用它占位）。
        let status =
            llm_set_key_with("deepseek", "set", &keys, NativePrompt::Ready(fake_prompt)).unwrap();
        assert!(
            keys.get_key("deepseek")
                .unwrap()
                .unwrap()
                .expose_secret()
                .ends_with("deepseek")
        );
        assert_eq!(status.hint.as_deref(), Some("seek"));
        // 原生输入失败：store.* 透传，诊断与载荷不含明文。
        let err = llm_set_key_with(
            "deepseek",
            "set",
            &keys,
            NativePrompt::Ready(|_label| {
                Err(mythos_store::error::StoreError::Io {
                    code: "io",
                    source: std::io::Error::other("credential ui failed"),
                })
            }),
        )
        .unwrap_err();
        assert_eq!(err.code(), "store.io");
        let serialized = serde_json::to_string(&err).unwrap();
        assert!(
            !serialized.contains("sk-new-1234"),
            "错误形状不得携带明文：{serialized}"
        );
    }

    #[test]
    fn unverified_platform_prompt_rejects_set_without_touching_store() {
        // macOS / Linux 的原生输入未验证：set 显式拒绝（app.bad-request），
        // 绝不降级为 Webview 明文提交，也不改动已存凭据。
        let keys = MemoryKeys::default();
        keys.set_key("deepseek", SecretString::from(String::from("sk-keep")))
            .unwrap();
        let err = llm_set_key_with("deepseek", "set", &keys, NativePrompt::Unverified).unwrap_err();
        assert_eq!(err.code(), "app.bad-request");
        assert_eq!(
            keys.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-keep"
        );
    }

    #[test]
    fn unknown_action_maps_to_bad_request() {
        let keys = MemoryKeys::default();
        // 注入器只是占位：未知 action 必须在任何交互前拒绝。
        #[cfg(windows)]
        let err = llm_set_key_with("deepseek", "bogus", &keys, NativePrompt::Ready(fake_prompt))
            .unwrap_err();
        #[cfg(not(windows))]
        let err =
            llm_set_key_with("deepseek", "bogus", &keys, NativePrompt::Unverified).unwrap_err();
        assert_eq!(err.code(), "app.bad-request");
    }

    #[test]
    fn status_command_reads_through_shared_vault() {
        // 命令级：对从未设置过的唯一 id 读状态 / 清除（NoEntry 幂等路径），
        // 不写入任何真实凭据。目录由先到的测试注入（OnceLock 全进程共享）。
        init_llm_dir(tdir("vault-probe"));
        let provider = unique("vault-probe");
        let status = llm_get_key_status(provider.clone()).unwrap();
        assert!(!status.set);
        let status = llm_set_key(provider, "clear".into()).unwrap();
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({ "set": false, "hint": null })
        );
    }
}
