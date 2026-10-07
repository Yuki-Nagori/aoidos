//! LLM profile：校验、持久化与冻结。
//!
//! profile 是可保存的用户配置（模型 / 调用形态 / 采样 / 代理 / thinking），
//! 提交时才冻结为 [`FrozenProfile`]——凭据在冻结瞬间从 [`CredentialStore`]
//! 取得不可变副本，回合内重试不能重新解析到热更新的密钥（llm.md：运行中换
//! 设置只影响下一回合）。持久化经 mythos-store 的原子写；文件形状是
//! `{"version":1,"profiles":[…]}`，后续字段演进走版本迁移，不改写既有存档。

use std::io::Read;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use mythos_store::atomic::write_text_atomic;
use mythos_store::error::{Result, StoreError};

use crate::credentials::CredentialStore;
use crate::sampling::Sampling;

/// 持久化文件内的顶层形状；version 字段驱动后续读取迁移。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileFile {
    version: u32,
    profiles: Vec<LlmProfile>,
}

/// 可保存的 LLM profile。字段与 llm.md「供应商能力与初始 profile」对齐；
/// 校验只做结构合法性（非空、枚举、URL 形态），能力匹配在提交时对
/// capabilities 检查（020 的职责，不在保存时拒绝）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LlmProfile {
    pub profile_id: String,
    /// 供应商标识，与 [`crate::providers::ProviderId::as_str`] 同源。
    pub provider_id: String,
    pub model: String,
    pub mode: ProfileMode,
    /// 叙事 adapter 显式关闭 thinking（llm.md）；v1 固定 false 由提交层消费。
    pub thinking: bool,
    pub sampling: Sampling,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

/// 调用形态：与 [`crate::provider::RequestMode`] 同语义的可持久化枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileMode {
    Completion,
    Chat,
}

impl From<ProfileMode> for crate::provider::RequestMode {
    fn from(mode: ProfileMode) -> Self {
        match mode {
            ProfileMode::Completion => Self::Completion,
            ProfileMode::Chat => Self::Chat,
        }
    }
}

/// 代理配置：三模式互斥由枚举天然表达（llm.md：system / none / manual
/// 不叠加；manual 禁用系统代理自动叠加）。凭据引用是 OS 凭据库里的条目名，
/// 不内嵌用户名密码。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProxyConfig {
    #[default]
    /// 启动时形成的环境代理快照；快照为空等价直连，但语义上保留「跟随系统」。
    System,
    /// 显式禁用一切代理。
    None,
    /// 显式 HTTP(S) / SOCKS5 代理；认证引用由凭据存储解析，不落配置文件。
    #[serde(rename_all = "camelCase")]
    Manual {
        /// 必须是 `http://`、`https://` 或 `socks5://`，禁止 URL 内嵌凭据。
        url: String,
        /// 凭据库中的代理认证条目（用户名 / 密码合并存储），可选。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_ref: Option<String>,
    },
}

/// profile 校验失败（结构非法，非能力失配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    /// 裸码：`invalid-path` 的同族结构错误用 `invalid-profile`，提交层映射
    /// app.bad-request。
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProfileError {}

impl LlmProfile {
    /// 结构校验：id / provider / model 非空且无控制字符，manual 代理 URL
    /// 形态合法且不含内嵌凭据。能力匹配（模型是否存在、温度范围）不在保存时
    /// 做——那是提交路径对 capabilities 的检查。
    ///
    /// # Errors
    /// 任一字段为空 / 含控制字符、或代理 URL 形态非法时返回 [`ProfileError`]。
    pub fn validate(&self) -> std::result::Result<(), ProfileError> {
        for (field, value) in [
            ("profileId", &self.profile_id),
            ("providerId", &self.provider_id),
            ("model", &self.model),
        ] {
            if value.trim().is_empty() {
                return Err(ProfileError {
                    code: "invalid-profile",
                    message: format!("{field} 不能为空"),
                });
            }
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(ProfileError {
                    code: "invalid-profile",
                    message: format!("{field} 过长或含控制字符"),
                });
            }
        }
        if !self.sampling.temperature.is_finite() || self.sampling.max_tokens == 0 {
            return Err(ProfileError {
                code: "invalid-profile",
                message: "采样参数必须有限且输出上限为正数".into(),
            });
        }
        if let ProxyConfig::Manual { url, auth_ref } = &self.proxy {
            validate_proxy_url(url)?;
            if auth_ref.as_ref().is_some_and(|id| {
                id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            }) {
                return Err(ProfileError {
                    code: "invalid-profile",
                    message: "代理凭据引用非法".into(),
                });
            }
        }
        Ok(())
    }
}

/// 代理 URL 解析失败（结构校验，非能力失配）。
fn err_proxy_url(_: url::ParseError) -> ProfileError {
    ProfileError {
        code: "invalid-profile",
        message: "代理 URL 不是合法地址".into(),
    }
}

/// 代理 URL 校验：scheme 限 http/https/socks5，host 非空，禁止 userinfo
///（内嵌凭据永不进配置文件——凭据经 `auth_ref` 走凭据存储）。
fn validate_proxy_url(url: &str) -> std::result::Result<(), ProfileError> {
    if url.len() > 2048 || url.chars().any(char::is_control) {
        return Err(ProfileError {
            code: "invalid-profile",
            message: "代理地址过长或含控制字符".into(),
        });
    }
    let parsed = url::Url::parse(url).map_err(err_proxy_url)?;
    match parsed.scheme() {
        "http" | "https" | "socks5" => {}
        _ => {
            return Err(ProfileError {
                code: "invalid-profile",
                message: "代理 scheme 不支持".into(),
            });
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ProfileError {
            code: "invalid-profile",
            message: "代理 URL 不得内嵌用户名 / 密码；认证请使用凭据引用".into(),
        });
    }
    if parsed.host_str().is_none() {
        return Err(ProfileError {
            code: "invalid-profile",
            message: "代理 URL 缺少主机".into(),
        });
    }
    Ok(())
}

/// 冻结后的 profile：凭据是取出的不可变副本（SecretString），回合内重试
/// 复用同一副本；`clear` / 换 key 不影响已冻结回合（llm.md）。
pub struct FrozenProfile {
    pub profile: LlmProfile,
    pub credential: Option<SecretString>,
}

/// 提交时冻结：profile 深拷贝 + 凭据一次性取出。凭据未设置时返回
/// `llm.missing-key` 语义（这里以 `None` 表达，由提交层定码）。
///
/// # Errors
/// 凭据后端读取失败时按 store.* 透传。
pub fn freeze(profile: &LlmProfile, credentials: &dyn CredentialStore) -> Result<FrozenProfile> {
    Ok(FrozenProfile {
        profile: profile.clone(),
        credential: credentials.get_key(&profile.provider_id)?,
    })
}

/// 集合硬上限同时用于命令与磁盘校验。
pub const MAX_PROFILES: usize = 50;

/// profile 集合的持久化读写，固定文件名 profiles.json，读取上限 512 KiB。
pub struct ProfileStore {
    path: PathBuf,
}

impl ProfileStore {
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("profiles.json"),
        }
    }

    /// 保存整份 profile 集合（校验后原子覆写）。
    ///
    /// # Errors
    /// 任一 profile 校验失败（`corrupt`，原因在错误文本）、目录不可建或
    /// 原子写失败时返回对应 store.* 错误。
    pub fn save(&self, profiles: &[LlmProfile]) -> Result<()> {
        validate_profiles(profiles)?;
        // parent 为 None 仅在路径是根时出现，落盘会在写入时报错，这里统一建目录。
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))
            .map_err(StoreError::from_io)?;
        let file = ProfileFile {
            version: 1,
            profiles: profiles.to_vec(),
        };
        let text = serde_json::to_string(&file).map_err(err_profile_serialize)?;
        write_text_atomic(&self.path, &text)
    }

    /// 读取整份集合；文件缺失视为空（首次安装），损坏报 `corrupt`。
    ///
    /// # Errors
    /// 目录不可读（非缺失）或文件损坏时返回对应 store.* 错误。
    pub fn load(&self) -> Result<Vec<LlmProfile>> {
        match std::fs::File::open(&self.path) {
            Ok(file) => {
                let mut text = String::new();
                file.take(524_289)
                    .read_to_string(&mut text)
                    .map_err(StoreError::from_io)?;
                if text.len() > 524_288 {
                    return Err(StoreError::Corrupt("profiles file too large".into()));
                }
                let file: ProfileFile = serde_json::from_str(&text).map_err(err_profiles_json)?;
                if file.version != 1 {
                    return Err(StoreError::Corrupt("unsupported profiles version".into()));
                }
                validate_profiles(&file.profiles)?;
                Ok(file.profiles)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => missing_profiles(&self.path),
            Err(err) => Err(StoreError::from_io(err)),
        }
    }
}

// Windows 将文件父路径下的查找也映射为 NotFound；只有真正缺项才是首次安装。
fn missing_profiles(path: &Path) -> Result<Vec<LlmProfile>> {
    if path.parent().is_some_and(Path::is_file) {
        return Err(StoreError::InvalidPath(
            "profile parent is not a directory".into(),
        ));
    }
    Ok(Vec::new())
}

fn validate_profiles(profiles: &[LlmProfile]) -> Result<()> {
    if profiles.len() > MAX_PROFILES {
        return Err(StoreError::Corrupt("too many profiles".into()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for profile in profiles {
        profile.validate().map_err(profile_error)?;
        if !ids.insert(&profile.profile_id) {
            return Err(StoreError::Corrupt("duplicate profile id".into()));
        }
    }
    Ok(())
}

/// profile 集合序列化失败：字段类型受控，正常不可达（防御性映射）。
fn err_profile_serialize(_: serde_json::Error) -> StoreError {
    StoreError::Corrupt("profile serialization failed".into())
}

/// profiles.json 不是合法 JSON（外部篡改或损坏）。
fn err_profiles_json(_: serde_json::Error) -> StoreError {
    StoreError::Corrupt("profiles file is not valid json".into())
}

fn profile_error(error: ProfileError) -> StoreError {
    StoreError::Corrupt(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryStore;
    use crate::temp_test_dir;
    use secrecy::ExposeSecret;

    fn deepseek_profile() -> LlmProfile {
        LlmProfile {
            profile_id: "narration".into(),
            provider_id: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
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
    fn loaded_profiles_validate_version_shape_and_set_invariants() {
        let dir = temp_test_dir("validated-read");
        let path = dir.join("profiles.json");
        let store = ProfileStore::new(&dir);
        let profile = deepseek_profile();
        for text in [
            serde_json::json!({"version": 2, "profiles": []}).to_string(),
            serde_json::json!({"version": 1, "profiles": [profile, profile]}).to_string(),
            serde_json::json!({"version": 1, "profiles": (0..51).map(|i| { let mut profile = deepseek_profile(); profile.profile_id = format!("p{i}"); profile }).collect::<Vec<_>>()}).to_string(),
            serde_json::json!({"version": 1, "profiles": [], "apiKey": "must-not-be-accepted"})
                .to_string(),
            serde_json::json!({"version":1,"profiles":[]}).to_string() + &" ".repeat(524_289),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(store.load().unwrap_err().code(), "corrupt");
        }
        let mut invalid = deepseek_profile();
        invalid.model.clear();
        std::fs::write(
            &path,
            serde_json::json!({"version":1,"profiles":[invalid]}).to_string(),
        )
        .unwrap();
        assert_eq!(store.load().unwrap_err().code(), "corrupt");
        assert!(store.save(&[profile.clone(), profile]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_sampling_and_credential_references_fail_before_persistence() {
        let mut profile = deepseek_profile();
        profile.sampling.temperature = f64::NAN;
        assert!(profile.validate().is_err());
        profile.sampling.temperature = 1.0;
        profile.sampling.max_tokens = 0;
        assert!(profile.validate().is_err());
        profile.sampling.max_tokens = 1;
        profile.model = "x".repeat(257);
        assert!(profile.validate().is_err());
        profile.model = "m".into();
        for url in ["x".repeat(2049), "http://p\n".into()] {
            profile.proxy = ProxyConfig::Manual {
                url,
                auth_ref: None,
            };
            assert!(profile.validate().is_err());
        }
        for reference in [" ".into(), "x".repeat(129), "a\n".into()] {
            profile.proxy = ProxyConfig::Manual {
                url: "http://p:1".into(),
                auth_ref: Some(reference),
            };
            assert!(profile.validate().is_err());
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let store = ProfileStore::new(&temp_test_dir("rt"));
        let profile = deepseek_profile();
        store.save(std::slice::from_ref(&profile)).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, vec![profile]);
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let store = ProfileStore::new(&temp_test_dir("missing"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn defensive_json_mappers_map_to_corrupt() {
        // 防御性映射正常不可达（受控类型不会序列化失败），按映射器直测约定覆盖。
        let json_error = || serde_json::from_str::<()>("garbage").unwrap_err();
        assert_eq!(err_profiles_json(json_error()).code(), "corrupt");
        assert_eq!(err_profile_serialize(json_error()).code(), "corrupt");
    }

    #[test]
    fn corrupt_file_maps_to_corrupt() {
        let dir = temp_test_dir("corrupt");
        std::fs::write(dir.join("profiles.json"), b"garbage").unwrap();
        assert_eq!(
            ProfileStore::new(&dir).load().unwrap_err().code(),
            "corrupt"
        );
    }

    #[test]
    fn validation_rejects_empty_and_control_chars() {
        let mut profile = deepseek_profile();
        profile.model = "  ".into();
        assert_eq!(profile.validate().unwrap_err().code, "invalid-profile");
        profile.model = "a\u{0}b".into();
        assert!(profile.validate().is_err());
    }

    #[test]
    fn validation_rejects_bad_proxy_urls() {
        let mut profile = deepseek_profile();
        profile.proxy = ProxyConfig::Manual {
            url: "ftp://proxy:21".into(),
            auth_ref: None,
        };
        assert!(profile.validate().is_err());
        profile.proxy = ProxyConfig::Manual {
            url: "http://user:pass@proxy:8080".into(),
            auth_ref: None,
        };
        // 内嵌凭据必须被拒绝（凭据走 auth_ref）。
        assert!(profile.validate().unwrap_err().message.contains("凭据"));
        profile.proxy = ProxyConfig::Manual {
            url: "socks5://127.0.0.1:1080".into(),
            auth_ref: Some("proxy-auth".into()),
        };
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn freeze_captures_credential_once_and_retries_do_not_reread() {
        let credentials = MemoryStore::default();
        credentials
            .set_key("deepseek", SecretString::from(String::from("sk-first")))
            .unwrap();
        let frozen = freeze(&deepseek_profile(), &credentials).unwrap();
        // 冻结后换 key / clear 不影响已冻结副本。
        credentials
            .set_key("deepseek", SecretString::from(String::from("sk-second")))
            .unwrap();
        assert_eq!(
            frozen.credential.as_ref().unwrap().expose_secret(),
            "sk-first"
        );
        credentials.clear_key("deepseek").unwrap();
        assert_eq!(
            frozen.credential.as_ref().unwrap().expose_secret(),
            "sk-first"
        );
    }

    #[test]
    fn freeze_without_key_is_none_not_error() {
        let credentials = MemoryStore::default();
        let frozen = freeze(&deepseek_profile(), &credentials).unwrap();
        assert!(frozen.credential.is_none());
    }

    #[test]
    fn freeze_propagates_backend_failure() {
        // 凭据后端不可用：冻结失败按 store.* 透传，不静默当未设置。
        let credentials = FailingBackend;
        assert!(
            credentials
                .set_key("x", SecretString::from(String::from("v")))
                .is_err()
        );
        assert!(credentials.clear_key("x").is_err());
        assert!(freeze(&deepseek_profile(), &credentials).is_err());
    }

    #[test]
    fn save_reports_uncreatable_directory() {
        // 父路径被文件占据：建目录失败如实上报。
        let dir = temp_test_dir("save-blocked");
        std::fs::write(dir.join("blocker"), b"x").unwrap();
        let store = ProfileStore::new(&dir.join("blocker").join("sub"));
        assert!(store.save(&Vec::new()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 永远失败的凭据后端（与 credentials 测试的同构夹具，跨模块不共享）。
    struct FailingBackend;

    impl FailingBackend {
        fn fail<T>() -> Result<T> {
            Err(StoreError::Corrupt("backend unavailable".into()))
        }
    }

    impl CredentialStore for FailingBackend {
        fn set_key(&self, _provider_id: &str, _secret: SecretString) -> Result<()> {
            Self::fail()
        }

        fn get_key(&self, _provider_id: &str) -> Result<Option<SecretString>> {
            Self::fail()
        }

        fn clear_key(&self, _provider_id: &str) -> Result<()> {
            Self::fail()
        }
    }

    #[test]
    fn serialized_profile_contains_no_secrets() {
        let profile = deepseek_profile();
        let json = serde_json::to_string(&profile).unwrap();
        // 配置文件形状里只有代理引用名，没有任何密钥字段。
        assert!(!json.contains("apiKey") && !json.contains("secret"));
    }

    #[test]
    fn deserialization_rejects_unknown_shapes() {
        // 未知枚举值 / 缺字段：持久化读取路径必须显式报错，不得吞成默认值。
        assert!(serde_json::from_str::<ProxyConfig>("{\"mode\":\"bogus\"}").is_err());
        assert!(serde_json::from_str::<ProxyConfig>("{}").is_err());
        assert!(serde_json::from_str::<ProfileMode>("\"bogus\"").is_err());
        assert!(serde_json::from_str::<Sampling>("{\"temperature\":1.0}").is_err());
        assert!(
            serde_json::from_str::<LlmProfile>("{\"profileId\":\"p\",\"providerId\":\"deepseek\"}")
                .is_err()
        );
    }

    #[test]
    fn proxy_config_serialization_is_tagged_enum() {
        let system = ProxyConfig::System;
        assert_eq!(
            serde_json::to_string(&system).unwrap(),
            r#"{"mode":"system"}"#
        );
        let none = ProxyConfig::None;
        assert_eq!(serde_json::to_string(&none).unwrap(), r#"{"mode":"none"}"#);
        let manual = ProxyConfig::Manual {
            url: "http://p:1".into(),
            auth_ref: None,
        };
        assert_eq!(
            serde_json::to_string(&manual).unwrap(),
            r#"{"mode":"manual","url":"http://p:1"}"#
        );
        // 回读保持同形态。
        let back: ProxyConfig =
            serde_json::from_str(&serde_json::to_string(&manual).unwrap()).unwrap();
        assert_eq!(back, manual);
    }

    #[test]
    fn profile_mode_maps_to_request_mode() {
        assert_eq!(
            crate::provider::RequestMode::from(ProfileMode::Completion),
            crate::provider::RequestMode::Completion
        );
        assert_eq!(
            crate::provider::RequestMode::from(ProfileMode::Chat),
            crate::provider::RequestMode::Chat
        );
    }

    #[test]
    fn validation_rejects_unparseable_and_hostless_proxy_urls() {
        let mut profile = deepseek_profile();
        // 完全不可解析。
        profile.proxy = ProxyConfig::Manual {
            url: "not a url".into(),
            auth_ref: None,
        };
        let error = profile.validate().unwrap_err();
        assert_eq!(error.code, "invalid-profile");
        assert!(error.message.contains("合法地址"));
        // scheme 合法但无主机（socks5 是非 special scheme，可无 authority）。
        profile.proxy = ProxyConfig::Manual {
            url: "socks5:nohost".into(),
            auth_ref: None,
        };
        assert!(profile.validate().unwrap_err().message.contains("主机"));
    }

    #[test]
    fn save_rejects_invalid_profiles_as_corrupt() {
        let store = ProfileStore::new(&temp_test_dir("save-invalid"));
        let mut profile = deepseek_profile();
        profile.model = " ".into();
        assert_eq!(
            store
                .save(std::slice::from_ref(&profile))
                .unwrap_err()
                .code(),
            "corrupt"
        );
    }

    #[test]
    fn load_maps_non_not_found_io_errors() {
        // 中间路径是普通文件：open 失败不是 NotFound，走 io_error 映射
        //（Windows 上读目录为 PermissionDenied，Linux 侧同断言走各平台码）。
        let dir = temp_test_dir("load-dir");
        std::fs::write(dir.join("blocker"), b"x").unwrap();
        assert_eq!(
            missing_profiles(&dir.join("blocker").join("profiles.json"))
                .unwrap_err()
                .code(),
            "invalid-path"
        );
        let error = ProfileStore::new(&dir.join("blocker")).load().unwrap_err();
        assert_ne!(error.code(), "not-found");
        assert_ne!(error.code(), "corrupt");
        std::fs::remove_file(dir.join("blocker")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
