//! 应用层管理 profile 与凭据的一致性边界，入口只负责用户交互。

use aoidos_engine::{fault::Fault, game::product::ResolvedProfile};
use aoidos_llm::{
    config::{self, FrozenProfile, LlmProfile, MAX_PROFILES, ProfileError, ProfileStore},
    credentials::{CredentialStore, CredentialVault, KeyStatus},
    proxy,
};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Profile 文件与密钥库的应用服务；一实例对应一个应用数据根。
pub struct Profiles {
    directory: PathBuf,
    profile_lock: Mutex<()>,
    credential_lock: Mutex<()>,
    credentials: Box<dyn CredentialStore>,
}

impl Profiles {
    /// 使用共享 LLM 配置和 OS 优先凭据后端创建应用服务。
    #[must_use]
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
            profile_lock: Mutex::new(()),
            credential_lock: Mutex::new(()),
            credentials: Box::new(CredentialVault::new(directory)),
        }
    }

    #[cfg(test)]
    fn with_credentials(directory: &Path, credentials: Box<dyn CredentialStore>) -> Self {
        Self {
            directory: directory.to_path_buf(),
            profile_lock: Mutex::new(()),
            credential_lock: Mutex::new(()),
            credentials,
        }
    }

    /// 读取已保存的 profile。
    ///
    /// # Errors
    /// 文件不可读或数据损坏时返回脱敏存储 Fault。
    pub fn list(&self) -> Result<Vec<LlmProfile>, Fault> {
        let _guard = self
            .profile_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.store().load().map_err(store_fault)
    }

    /// 新增或覆盖一个 profile，并执行结构与集合容量校验。
    ///
    /// # Errors
    /// profile 非法、数量超限或持久化失败时返回稳定 Fault。
    pub fn save(&self, profile: LlmProfile) -> Result<LlmProfile, Fault> {
        profile.validate().map_err(invalid_profile)?;
        let _guard = self
            .profile_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let store = self.store();
        let mut profiles = store.load().map_err(store_fault)?;
        if let Some(existing) = profiles
            .iter_mut()
            .find(|item| item.profile_id == profile.profile_id)
        {
            *existing = profile.clone();
        } else {
            if profiles.len() >= MAX_PROFILES {
                return Err(Fault::new(
                    "app.bad-request",
                    format!("profile 数量已达上限 {MAX_PROFILES}"),
                ));
            }
            profiles.push(profile.clone());
        }
        store.save(&profiles).map_err(store_fault)?;
        Ok(profile)
    }

    /// 删除 profile；不存在时也成功，保持幂等。
    ///
    /// # Errors
    /// 文件读取或写入失败时返回稳定 Fault。
    pub fn delete(&self, profile_id: &str) -> Result<(), Fault> {
        let _guard = self
            .profile_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let store = self.store();
        let mut profiles = store.load().map_err(store_fault)?;
        let original_len = profiles.len();
        profiles.retain(|profile| profile.profile_id != profile_id);
        if profiles.len() != original_len {
            store.save(&profiles).map_err(store_fault)?;
        }
        Ok(())
    }

    /// 在提交边界冻结 profile、模型密钥和代理认证副本。
    ///
    /// # Errors
    /// profile 不存在、密钥库不可读或代理认证引用损坏时拒绝提交。
    pub fn freeze(&self, profile_id: &str) -> Result<ResolvedProfile, Fault> {
        config::validate_identifier("profileId", profile_id).map_err(invalid_profile)?;
        let profile = {
            let _guard = self
                .profile_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.store()
                .load()
                .map_err(store_fault)?
                .into_iter()
                .find(|profile| profile.profile_id == profile_id)
                .ok_or_else(profile_missing)?
        };

        let _guard = self
            .credential_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frozen: FrozenProfile =
            config::freeze(&profile, self.credentials.as_ref()).map_err(store_fault)?;
        let auth_ref = match &profile.proxy {
            config::ProxyConfig::Manual { auth_ref, .. } => auth_ref.as_deref(),
            config::ProxyConfig::System | config::ProxyConfig::None => None,
        };
        let proxy_auth =
            proxy::resolve_proxy_auth(self.credentials.as_ref(), auth_ref).map_err(store_fault)?;
        Ok(ResolvedProfile { frozen, proxy_auth })
    }

    /// 查询密钥状态，只返回脱敏 hint。
    ///
    /// # Errors
    /// provider 标识无效或凭据后端不可读时返回 Fault。
    pub fn key_status(&self, provider_id: &str) -> Result<KeyStatus, Fault> {
        config::validate_identifier("providerId", provider_id).map_err(invalid_profile)?;
        let _guard = self
            .credential_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.credentials.status(provider_id).map_err(store_fault)
    }

    /// 写入密钥。明文只在此方法参数及 OS / 降级凭据后端之间流动。
    ///
    /// # Errors
    /// provider 标识无效或凭据后端不可写时返回 Fault。
    pub fn set_key(&self, provider_id: &str, secret: secrecy::SecretString) -> Result<(), Fault> {
        config::validate_identifier("providerId", provider_id).map_err(invalid_profile)?;
        let _guard = self
            .credential_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.credentials
            .set_key(provider_id, secret)
            .map_err(store_fault)
    }

    /// 删除密钥；重复删除保持成功。
    ///
    /// # Errors
    /// provider 标识无效或凭据后端不可写时返回 Fault。
    pub fn clear_key(&self, provider_id: &str) -> Result<(), Fault> {
        config::validate_identifier("providerId", provider_id).map_err(invalid_profile)?;
        let _guard = self
            .credential_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.credentials.clear_key(provider_id).map_err(store_fault)
    }

    /// 原子删除密钥并读取删除后的状态，避免并发写入插入两步操作之间。
    ///
    /// # Errors
    /// provider 标识无效或凭据后端不可读、不可写时返回 Fault。
    pub fn clear_key_and_status(&self, provider_id: &str) -> Result<KeyStatus, Fault> {
        config::validate_identifier("providerId", provider_id).map_err(invalid_profile)?;
        let _guard = self
            .credential_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.credentials
            .clear_key(provider_id)
            .and_then(|()| self.credentials.status(provider_id))
            .map_err(store_fault)
    }

    fn store(&self) -> ProfileStore {
        ProfileStore::new(&self.directory)
    }
}

impl aoidos_engine::game::product::ProfileSource for Profiles {
    fn freeze(&self, profile_id: &str) -> Result<ResolvedProfile, Fault> {
        Profiles::freeze(self, profile_id)
    }
}

fn invalid_profile(error: ProfileError) -> Fault {
    Fault::new("app.bad-request", error.to_string())
}

fn profile_missing() -> Fault {
    Fault::new("app.not-found", "配置不存在")
}

fn store_fault(error: aoidos_store::error::StoreError) -> Fault {
    let code = error.code();
    let detail = match &error {
        aoidos_store::error::StoreError::InvalidPath(path) => {
            Some(serde_json::json!({ "path": path }))
        }
        aoidos_store::error::StoreError::AlreadyRunning { lock_path } => {
            Some(serde_json::json!({ "lockPath": lock_path.display().to_string() }))
        }
        aoidos_store::error::StoreError::LockedTimeout { path } => {
            Some(serde_json::json!({ "path": path.display().to_string() }))
        }
        aoidos_store::error::StoreError::Migration { version, source } => {
            Some(serde_json::json!({ "version": version, "reason": source.to_string() }))
        }
        aoidos_store::error::StoreError::Corrupt(reason) => {
            Some(serde_json::json!({ "reason": reason }))
        }
        aoidos_store::error::StoreError::Io { .. } => None,
    };
    let message = match code {
        "invalid-path" => "路径不合法",
        "already-running" => "应用已在运行",
        "locked" => "目标被占用，请稍后重试",
        "migration" => "数据库迁移失败",
        "disk-full" => "磁盘空间不足",
        "permission" => "没有操作权限",
        "not-found" => "文件或目录不存在",
        "corrupt" => "存储数据损坏",
        _ => "存储读写失败",
    };
    let fault = Fault::new(format!("store.{code}"), message);
    match detail {
        Some(detail) => fault.with_detail(detail),
        None => fault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoidos_llm::{
        config::{ProfileMode, ProxyConfig},
        sampling::Sampling,
    };
    use secrecy::ExposeSecret;
    use std::{
        collections::BTreeMap,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    #[derive(Default)]
    struct MemoryCredentials(Mutex<BTreeMap<String, String>>);

    impl CredentialStore for MemoryCredentials {
        fn set_key(
            &self,
            provider_id: &str,
            secret: secrecy::SecretString,
        ) -> aoidos_store::error::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(provider_id.to_owned(), secret.expose_secret().to_owned());
            Ok(())
        }

        fn get_key(
            &self,
            provider_id: &str,
        ) -> aoidos_store::error::Result<Option<secrecy::SecretString>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(provider_id)
                .map(|secret| secrecy::SecretString::from(secret.clone())))
        }

        fn clear_key(&self, provider_id: &str) -> aoidos_store::error::Result<()> {
            self.0.lock().unwrap().remove(provider_id);
            Ok(())
        }
    }

    fn temp_dir() -> std::path::PathBuf {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

        loop {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "aoidos-app-profiles-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&directory) {
                Ok(()) => return directory,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("创建 profile 测试目录失败：{error}"),
            }
        }
    }

    fn profile() -> LlmProfile {
        LlmProfile {
            profile_id: "narration".into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            mode: ProfileMode::Completion,
            thinking: false,
            sampling: Sampling {
                temperature: 1.0,
                max_tokens: 2048,
            },
            proxy: ProxyConfig::System,
        }
    }

    #[test]
    fn profile_lifecycle_and_freezing_share_validated_application_service() {
        let directory = temp_dir();
        let service = Profiles::with_credentials(&directory, Box::<MemoryCredentials>::default());
        assert!(service.list().unwrap().is_empty());
        let mut saved = profile();
        saved.model = "deepseek-v4-pro".into();
        service.save(saved).unwrap();
        let mut replacement = profile();
        replacement.model = "deepseek-v4-flash".into();
        service.save(replacement).unwrap();
        assert_eq!(service.list().unwrap().len(), 1);
        service
            .set_key("deepseek", secrecy::SecretString::from("sk-example-secret"))
            .unwrap();

        let frozen = service.freeze("narration").unwrap();
        assert_eq!(frozen.frozen.profile.model, "deepseek-v4-flash");
        assert_eq!(
            frozen.frozen.credential.unwrap().expose_secret(),
            "sk-example-secret"
        );
        assert!(service.key_status("deepseek").unwrap().set);
        assert!(!service.clear_key_and_status("deepseek").unwrap().set);
        service.delete("narration").unwrap();
        service.delete("narration").unwrap();
        assert!(service.list().unwrap().is_empty());
        assert_eq!(
            service.freeze("narration").err().unwrap().code,
            "app.not-found"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn profile_save_rejects_invalid_data_before_persisting() {
        let directory = temp_dir();
        let service = Profiles::with_credentials(&directory, Box::<MemoryCredentials>::default());
        let mut invalid = profile();
        invalid.model = "  ".into();

        assert_eq!(service.save(invalid).unwrap_err().code, "app.bad-request");
        assert!(service.list().unwrap().is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn store_fault_preserves_safe_structured_context() {
        let invalid_path = store_fault(aoidos_store::error::StoreError::InvalidPath(
            "unsafe/name".into(),
        ));
        assert_eq!(invalid_path.code, "store.invalid-path");
        assert_eq!(invalid_path.detail.unwrap()["path"], "unsafe/name");

        let corrupt = store_fault(aoidos_store::error::StoreError::Corrupt(
            "数据库版本高于当前程序".into(),
        ));
        assert_eq!(corrupt.code, "store.corrupt");
        assert_eq!(corrupt.detail.unwrap()["reason"], "数据库版本高于当前程序");
    }

    #[test]
    fn profile_limit_applies_to_inserts_but_not_replacements() {
        let directory = temp_dir();
        let service = Profiles::with_credentials(&directory, Box::<MemoryCredentials>::default());
        for index in 0..MAX_PROFILES {
            let mut profile = profile();
            profile.profile_id = format!("p{index}");
            service.save(profile).unwrap();
        }

        let mut replacement = profile();
        replacement.profile_id = "p0".into();
        service.save(replacement).unwrap();
        let mut overflow = profile();
        overflow.profile_id = "overflow".into();
        assert_eq!(service.save(overflow).unwrap_err().code, "app.bad-request");
        assert_eq!(service.list().unwrap().len(), MAX_PROFILES);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
