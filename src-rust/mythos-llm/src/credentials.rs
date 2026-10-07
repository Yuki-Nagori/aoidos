//! 凭据存储：OS 凭据库优先（keyring crate v4 默认特性：Windows Credential
//! Manager / macOS Keychain / Linux Secret Service），不可用时降级为仅当前
//! 用户可读的私有文件（Unix 0600；Windows 用只含当前用户的受保护 DACL，
//! 不把 0600 当 Windows 权限）。
//!
//! 契约（ipc-contract.md 密钥隔离）：IPC 只暴露 `{ set, hint }`；hint 是密钥末尾
//! 4 个字符，短于 4 个字符时为空；明文不进前端状态、日志、错误 detail、Debug。
//! 读取 / 写入失败按 store.* 映射，不伪装为已设置。

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use mythos_store::atomic::write_atomic_private;
use mythos_store::error::{Result, StoreError};
use mythos_store::read::read_text_bounded;

/// 凭据库按 provider 存取密钥；句柄可克隆共享，019 内由命令层持有。
pub trait CredentialStore: Send + Sync {
    /// 保存密钥（存在即覆盖）。明文只进参数，不进返回值 / 日志。
    ///
    /// # Errors
    /// 后端不可写（OS 库与降级文件都失败）时返回 store.* 错误。
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()>;

    /// 读取密钥；未设置返回 `None`。
    ///
    /// # Errors
    /// 后端不可读时返回 store.* 错误；「未设置」不是错误。
    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>>;

    /// 删除密钥；未设置时也返回成功（幂等）。
    ///
    /// # Errors
    /// 后端不可写时返回 store.* 错误。
    fn clear_key(&self, provider_id: &str) -> Result<()>;

    /// 契约的设置状态：`set` 与 hint（末 4 字符；短于 4 为空）。
    ///
    /// # Errors
    /// 后端不可读时返回 store.* 错误。
    fn status(&self, provider_id: &str) -> Result<KeyStatus> {
        match self.get_key(provider_id)? {
            None => Ok(KeyStatus {
                set: false,
                hint: None,
            }),
            Some(secret) => Ok(KeyStatus {
                set: true,
                hint: hint_of(secret.expose_secret()),
            }),
        }
    }
}

/// IPC 暴露的凭据状态；序列化形状即契约的 `{ set, hint }`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub set: bool,
    pub hint: Option<String>,
}

/// hint 规则：末 4 个字符；短于 4 返回 `None`（只报告已设置）。
pub(crate) fn hint_of(secret: &str) -> Option<String> {
    let mut chars: Vec<char> = secret.chars().rev().take(4).collect();
    if chars.len() < 4 {
        None
    } else {
        chars.reverse();
        Some(chars.iter().collect())
    }
}

/// OS 凭据库（keyring 默认特性，见 tech-stack）。service 固定为应用名，user 为 providerId。
pub struct OsKeyring {
    service: &'static str,
    entry_factory: fn(&str, &str) -> keyring::Result<keyring::Entry>,
}

impl OsKeyring {
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: "mythos",
            entry_factory: keyring::Entry::new,
        }
    }

    fn entry(&self, provider_id: &str) -> Result<keyring::Entry> {
        (self.entry_factory)(self.service, provider_id).map_err(keyring_error)
    }
}

impl Default for OsKeyring {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for OsKeyring {
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
        self.entry(provider_id)?
            .set_password(secret.expose_secret())
            .map_err(keyring_error)
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        map_entry_password(self.entry(provider_id)?.get_password())
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        map_entry_clear(self.entry(provider_id)?.delete_credential())
    }
}

/// keyring 读结果 → 语义值（NoEntry 是「未设置」不是错误）；映射直测参照
/// mythos-store 的 err_* 模式。
fn map_entry_password(result: keyring::Result<String>) -> Result<Option<SecretString>> {
    match result {
        Ok(secret) => Ok(Some(SecretString::from(secret))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(keyring_error(error)),
    }
}

/// keyring 删除结果 → 语义值（未设置视为已清除，幂等）。
fn map_entry_clear(result: keyring::Result<()>) -> Result<()> {
    match result {
        // 未设置视为已清除（幂等）。
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_error(error)),
    }
}

/// keyring 错误 → store 域 `io`：不携带明文；NoEntry 走调用方 None 分支。
fn keyring_error(error: keyring::Error) -> StoreError {
    let reason = match error {
        keyring::Error::Ambiguous(_) => "ambiguous credential",
        _ => "credential store error",
    };
    StoreError::Io {
        code: "io",
        source: io::Error::other(reason),
    }
}

/// credentials.json 不是合法 JSON（外部篡改或损坏）。
fn err_credentials_json(_: serde_json::Error) -> StoreError {
    StoreError::Corrupt("credentials file is not valid json".into())
}

/// 凭据条目序列化失败：BTreeMap<String, String> 受控，正常不可达（防御性映射）。
fn err_credentials_serialize(_: serde_json::Error) -> StoreError {
    StoreError::Corrupt("credentials serialization failed".into())
}

/// 降级文件：`<dir>/credentials.json`，键为 providerId。整文件经
/// [`write_atomic_private`] 覆写；空临时文件先收紧为仅当前用户（Unix 0600 /
/// Windows 仅当前用户的受保护 DACL），已存在的宽松文件也会被重新收紧。
pub struct CredentialFile {
    path: PathBuf,
}

impl CredentialFile {
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("credentials.json"),
        }
    }

    fn read_data(&self) -> Result<CredentialData> {
        let Some(text) = read_text_bounded(&self.path, 524_288)? else {
            return Ok(CredentialData::default());
        };
        let decoded: CredentialEncoding =
            serde_json::from_str(&text).map_err(err_credentials_json)?;
        match decoded {
            CredentialEncoding::Current(data) if data.version == 2 => Ok(data),
            CredentialEncoding::Current(_) => Err(StoreError::Corrupt(
                "unsupported credentials version".into(),
            )),
            CredentialEncoding::Legacy(map) => Ok(CredentialData {
                version: 2,
                entries: map
                    .into_iter()
                    .map(|(id, value)| (id, CredentialEntry::File { value }))
                    .collect(),
            }),
        }
    }

    fn write_data(&self, data: &CredentialData) -> Result<()> {
        let text = serde_json::to_string(data).map_err(err_credentials_serialize)?;
        if text.len() > 524_288 {
            return Err(StoreError::Corrupt("credentials file too large".into()));
        }
        write_atomic_private(
            &self.path,
            text.as_bytes(),
            crate::platform::restrict_to_current_user,
        )
    }

    fn mark(&self, provider_id: &str, entry: CredentialEntry) -> Result<()> {
        let mut data = self.read_data()?;
        data.entries.insert(provider_id.to_owned(), entry);
        self.write_data(&data)
    }
}

impl CredentialStore for CredentialFile {
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
        self.mark(
            provider_id,
            CredentialEntry::File {
                value: secret.expose_secret().to_owned(),
            },
        )
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        Ok(match self.read_data()?.entries.remove(provider_id) {
            Some(CredentialEntry::File { value }) => Some(SecretString::from(value)),
            _ => None,
        })
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        let mut data = self.read_data()?;
        data.entries.remove(provider_id);
        if data.entries.is_empty() {
            clear_file(&self.path)
        } else {
            self.write_data(&data)
        }
    }
}

/// 不含明文的 OS 指针与删除墓碑，和降级密钥在同一次原子发布中提交。
#[derive(Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "camelCase", deny_unknown_fields)]
enum CredentialEntry {
    Os,
    File { value: String },
    Deleted,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialData {
    version: u32,
    entries: BTreeMap<String, CredentialEntry>,
}
impl Default for CredentialData {
    fn default() -> Self {
        Self {
            version: 2,
            entries: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CredentialEncoding {
    Current(CredentialData),
    Legacy(BTreeMap<String, String>),
}

/// 删除降级文件：缺失视为已删除（幂等）；其余失败如实上报。
fn clear_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(StoreError::from_io(err)),
    }
}

/// 写入优先 OS 凭据库；原子保存每个条目的当前权威后端。
/// OS 写入成功后文件只保留指针，写失败则保存文件值；读取按指针取值，
/// 不用 OS 的旧值删除新降级值。删除先保存墓碑，再尽力清 OS 条目，
/// 后端删除失败仍报告错误，但旧值不会复活。
/// 旧版无版本字典按文件值读取，下次写入升级，不猜测另一后端是否更新。
pub struct CredentialVault {
    os: Box<dyn CredentialStore>,
    file: CredentialFile,
}

impl CredentialVault {
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            os: Box::new(OsKeyring::new()),
            file: CredentialFile::new(dir),
        }
    }

    /// 注入 OS 库后端：验证组合语义（降级 / 迁移清理）不依赖真实凭据服务。
    #[cfg(test)]
    fn with_backends(os: Box<dyn CredentialStore>, file: CredentialFile) -> Self {
        Self { os, file }
    }
}

impl CredentialStore for CredentialVault {
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
        match self.os.set_key(provider_id, secret.clone()) {
            Ok(()) => self.file.mark(provider_id, CredentialEntry::Os),
            Err(_) => self.file.set_key(provider_id, secret),
        }
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        match self.file.read_data()?.entries.remove(provider_id) {
            Some(CredentialEntry::File { value }) => Ok(Some(SecretString::from(value))),
            Some(CredentialEntry::Deleted) => Ok(None),
            Some(CredentialEntry::Os) | None => self.os.get_key(provider_id),
        }
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        // 先提交墓碑，防止 OS 删除失败或进程中断后旧密钥再次被读出。
        self.file.mark(provider_id, CredentialEntry::Deleted)?;
        self.os.clear_key(provider_id)
    }
}

#[cfg(test)]
/// 测试用内存存储：注入确定性行为，不触 OS。模块级 pub(crate) 供
/// config 测试复用（冻结隔离测试的凭据后端）。
#[derive(Default)]
pub(crate) struct MemoryStore {
    keys: std::sync::Mutex<BTreeMap<String, String>>,
}

#[cfg(test)]
impl CredentialStore for MemoryStore {
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
        self.keys
            .lock()
            .unwrap()
            .insert(provider_id.to_owned(), secret.expose_secret().to_owned());
        Ok(())
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        Ok(self
            .keys
            .lock()
            .unwrap()
            .get(provider_id)
            .map(|raw| SecretString::from(raw.clone())))
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        self.keys.lock().unwrap().remove(provider_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_test_dir;

    struct ReadOnlyOldKey;
    impl CredentialStore for ReadOnlyOldKey {
        fn set_key(&self, _: &str, _: SecretString) -> Result<()> {
            FailingKeys::fail()
        }
        fn get_key(&self, _: &str) -> Result<Option<SecretString>> {
            Ok(Some(SecretString::from("sk-old-os".to_owned())))
        }
        fn clear_key(&self, _: &str) -> Result<()> {
            FailingKeys::fail()
        }
    }

    #[test]
    fn os_adapter_operations_are_testable_without_a_desktop_service() {
        fn entry(service: &str, user: &str) -> keyring::Result<keyring::Entry> {
            let credential = keyring_core::mock::Cred {
                specifiers: (service.into(), user.into()),
                inner: std::sync::Mutex::new(std::cell::RefCell::new(
                    keyring_core::mock::CredData::default(),
                )),
            };
            Ok(keyring::Entry {
                inner: keyring_core::Entry::new_with_credential(std::sync::Arc::new(credential)),
            })
        }
        let defaults = OsKeyring::default();
        assert_eq!(defaults.service, "mythos");
        let mut store = OsKeyring {
            entry_factory: entry,
            ..defaults
        };
        store
            .set_key("p", SecretString::from("fixture".to_owned()))
            .unwrap();
        assert!(store.get_key("p").unwrap().is_none());
        store.clear_key("p").unwrap();
        store.entry_factory = |_, _| Err(keyring::Error::NoDefaultStore);
        assert!(
            store
                .set_key("p", SecretString::from("fixture".to_owned()))
                .is_err()
        );
    }

    #[test]
    fn fallback_update_and_delete_never_resurrect_old_os_key() {
        let dir = temp_test_dir("authority");
        let vault =
            CredentialVault::with_backends(Box::new(ReadOnlyOldKey), CredentialFile::new(&dir));
        assert_eq!(
            vault.get_key("p").unwrap().unwrap().expose_secret(),
            "sk-old-os"
        );
        vault
            .set_key("p", SecretString::from("sk-new-file".to_owned()))
            .unwrap();
        assert_eq!(
            vault.get_key("p").unwrap().unwrap().expose_secret(),
            "sk-new-file"
        );
        assert!(vault.clear_key("p").is_err());
        let reopened =
            CredentialVault::with_backends(Box::new(ReadOnlyOldKey), CredentialFile::new(&dir));
        assert!(reopened.get_key("p").unwrap().is_none());
        assert!(
            !std::fs::read_to_string(dir.join("credentials.json"))
                .unwrap()
                .contains("sk-new-file")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn os_error_is_not_missing_and_legacy_files_keep_their_value() {
        let dir = temp_test_dir("legacy-authority");
        let vault =
            CredentialVault::with_backends(Box::new(FailingBackend), CredentialFile::new(&dir));
        assert!(vault.get_key("p").is_err());
        std::fs::write(dir.join("credentials.json"), r#"{"p":"sk-legacy"}"#).unwrap();
        assert_eq!(
            vault.get_key("p").unwrap().unwrap().expose_secret(),
            "sk-legacy"
        );
        vault.file.mark("p", CredentialEntry::Os).unwrap();
        assert!(vault.get_key("p").is_err());
        std::fs::write(
            dir.join("credentials.json"),
            r#"{"version":3,"entries":{}}"#,
        )
        .unwrap();
        assert_eq!(vault.get_key("p").unwrap_err().code(), "corrupt");
        std::fs::write(
            dir.join("credentials.json"),
            r#"{"version":2,"entries":{}}"#.to_owned() + &" ".repeat(524_289),
        )
        .unwrap();
        assert_eq!(vault.get_key("p").unwrap_err().code(), "corrupt");
        let data = CredentialData {
            version: 2,
            entries: BTreeMap::from([(
                "p".into(),
                CredentialEntry::File {
                    value: "x".repeat(524_289),
                },
            )]),
        };
        assert!(vault.file.write_data(&data).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hint_rules_match_contract() {
        assert_eq!(hint_of("sk-1234567890"), Some("7890".into()));
        assert_eq!(hint_of("abcd"), Some("abcd".into()));
        assert_eq!(hint_of("abc"), None);
        assert_eq!(hint_of(""), None);
        // 多字节字符按字符数计，不按字节切。
        assert_eq!(hint_of("密钥一二三四五"), Some("二三四五".into()));
    }

    #[test]
    fn keyring_result_mappers_cover_all_arms() {
        assert_eq!(
            map_entry_password(Ok(String::from("sk-x")))
                .unwrap()
                .unwrap()
                .expose_secret(),
            "sk-x"
        );
        assert!(
            map_entry_password(Err(keyring::Error::NoEntry))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            map_entry_password(Err(keyring::Error::BadStoreFormat("v".into())))
                .unwrap_err()
                .code(),
            "io"
        );
        assert!(map_entry_clear(Ok(())).is_ok());
        assert!(map_entry_clear(Err(keyring::Error::NoEntry)).is_ok());
        assert_eq!(
            map_entry_clear(Err(keyring::Error::BadStoreFormat("v".into())))
                .unwrap_err()
                .code(),
            "io"
        );
    }

    #[test]
    fn keyring_error_maps_to_store_io_without_plaintext() {
        // Ambiguous 与其余错误都归 store 域 io，诊断只含固定短语。
        let ambiguous = keyring_error(keyring::Error::Ambiguous(Vec::new()));
        assert_eq!(ambiguous.code(), "io");
        let other = keyring_error(keyring::Error::BadStoreFormat("secret-ish".into()));
        assert_eq!(other.code(), "io");
        let text = other.to_string();
        assert!(!text.contains("secret-ish"), "诊断不得携带原始串：{text}");
    }

    #[test]
    fn clear_file_is_idempotent_and_reports_real_failures() {
        // 缺失 = 已删除。
        let dir = temp_test_dir("clear-file");
        assert!(clear_file(&dir.join("absent.json")).is_ok());
        // 目标是目录：删除失败如实上报。
        std::fs::create_dir(dir.join("as-dir.json")).unwrap();
        assert!(clear_file(&dir.join("as-dir.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_credentials_file_maps_to_io_error() {
        // credentials.json 是目录：读取失败不是 NotFound，走 io_error；
        // 读路径（get / clear）都不得伪装成功。
        let dir = temp_test_dir("read-dir");
        std::fs::create_dir(dir.join("credentials.json")).unwrap();
        let store = CredentialFile::new(&dir);
        assert!(store.get_key("deepseek").is_err());
        assert!(store.clear_key("deepseek").is_err());
        std::fs::remove_dir(dir.join("credentials.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unwritable_directory_maps_to_io_error() {
        // 父路径被文件占据：建目录失败如实上报。
        let dir = temp_test_dir("write-blocked");
        std::fs::write(dir.join("blocker"), b"x").unwrap();
        let store = CredentialFile::new(&dir.join("blocker").join("sub"));
        assert!(
            store
                .set_key("deepseek", SecretString::from(String::from("sk-x")))
                .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn os_keyring_default_matches_new() {
        // Default 与 new 同构；对未设置过的 id 读状态走真实 NoEntry 路径。
        let store = OsKeyring::default();
        assert!(!store.status("mythos-test-default").unwrap().set);
    }

    #[test]
    fn status_shape_is_contract_json() {
        let store = MemoryStore::default();
        store
            .set_key("deepseek", SecretString::from(String::from("sk-12345678")))
            .unwrap();
        let status = store.status("deepseek").unwrap();
        assert_eq!(
            status,
            KeyStatus {
                set: true,
                hint: Some("5678".into())
            }
        );
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json, serde_json::json!({ "set": true, "hint": "5678" }));
        let missing = store.status("none").unwrap();
        let json = serde_json::to_value(&missing).unwrap();
        assert_eq!(json, serde_json::json!({ "set": false, "hint": null }));
    }

    #[test]
    fn status_reports_backend_failure_as_store_error() {
        // 后端不可用：status 报错而不是伪装「未设置」；内存后端正常返回。
        let failing = FailingKeys;
        assert!(
            failing
                .set_key("x", SecretString::from(String::from("v")))
                .is_err()
        );
        assert!(failing.clear_key("x").is_err());
        assert!(failing.status("deepseek").is_err());
        let keys = MemoryStore::default();
        assert!(keys.status("deepseek").is_ok());
    }

    /// 永远失败的凭据后端：status / 读取失败路径直测。
    struct FailingKeys;

    impl FailingKeys {
        fn fail<T>() -> Result<T> {
            Err(StoreError::Corrupt("backend unavailable".into()))
        }
    }

    impl CredentialStore for FailingKeys {
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
    fn file_store_round_trip_and_clear_removes_empty_file() {
        let dir = temp_test_dir("file");
        let store = CredentialFile::new(&dir);
        store
            .set_key("deepseek", SecretString::from(String::from("sk-abcdef")))
            .unwrap();
        assert_eq!(
            store.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-abcdef"
        );
        assert!(store.get_key("other").unwrap().is_none());
        // 多条目时清除其一：非空文件回写保留剩余条目。
        store
            .set_key("second", SecretString::from(String::from("sk-2")))
            .unwrap();
        store.clear_key("deepseek").unwrap();
        assert!(store.get_key("deepseek").unwrap().is_none());
        assert_eq!(
            store.get_key("second").unwrap().unwrap().expose_secret(),
            "sk-2"
        );
        // 幂等清除 + 空文件删除（最后一条删除后文件应移除）。
        store.clear_key("second").unwrap();
        store.clear_key("second").unwrap();
        assert!(store.get_key("second").unwrap().is_none());
        assert!(!dir.join("credentials.json").exists());
        // 未设置路径不创建目录外的任何东西。
        assert!(!store.status("deepseek").unwrap().set);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fallback_file_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_test_dir("perm-unix");
        let store = CredentialFile::new(&dir);
        store
            .set_key("deepseek", SecretString::from(String::from("sk-x")))
            .unwrap();
        let mode = std::fs::metadata(dir.join("credentials.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "降级文件必须仅当前用户可读写");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn fallback_file_acl_is_current_user_only() {
        let dir = temp_test_dir("perm-win");
        let store = CredentialFile::new(&dir);
        store
            .set_key("deepseek", SecretString::from(String::from("sk-x")))
            .unwrap();
        crate::platform::windows::assert_owner_only(&dir.join("credentials.json"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_falls_back_to_file_when_os_store_fails() {
        // OS 库不可用（写 / 读都报错）时写降级文件；读同样经文件找回。
        let dir = temp_test_dir("vault-fallback");
        let vault =
            CredentialVault::with_backends(Box::new(FailingBackend), CredentialFile::new(&dir));
        vault
            .set_key("deepseek", SecretString::from(String::from("sk-fallback")))
            .unwrap();
        assert_eq!(
            vault.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-fallback"
        );
        assert!(dir.join("credentials.json").exists(), "降级文件应存在");
        // clear 语义（任一后端失败即报错）：OS 库不可用时清除未确认完整，必须
        // 报错让用户知道 OS 侧可能仍持有条目；文件副本的清除已尽力完成。
        assert!(vault.clear_key("deepseek").is_err());
        assert!(vault.get_key("deepseek").unwrap().is_none());
        assert!(dir.join("credentials.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_migrates_off_the_file_copy_when_os_store_recovers() {
        // 先降级落文件，OS 库恢复后写入成功并清除文件副本（单向迁移）。
        let dir = temp_test_dir("vault-migrate");
        let file = CredentialFile::new(&dir);
        file.set_key("deepseek", SecretString::from(String::from("sk-old")))
            .unwrap();
        let os = MemoryStore::default();
        let vault = CredentialVault::with_backends(Box::new(os), file);
        vault
            .set_key("deepseek", SecretString::from(String::from("sk-new")))
            .unwrap();
        assert_eq!(
            vault.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-new"
        );
        assert!(
            !std::fs::read_to_string(dir.join("credentials.json"))
                .unwrap()
                .contains("sk-"),
            "OS 库可用后文件只保留后端指针，不得残留明文"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_read_falls_back_when_os_store_errors() {
        // OS 库瞬断（读报错）不能把已降级保存的密钥读丢。
        let dir = temp_test_dir("vault-read");
        let file = CredentialFile::new(&dir);
        file.set_key("deepseek", SecretString::from(String::from("sk-file")))
            .unwrap();
        let vault = CredentialVault::with_backends(Box::new(FailingBackend), file);
        assert_eq!(
            vault.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_keeps_confirmed_file_value_when_os_contains_another_key() {
        // 文件是最近确认的权威值；另一后端仍可读不意味着它更晚。
        let dir = temp_test_dir("vault-cleanup-retry");
        let file = CredentialFile::new(&dir);
        file.set_key("deepseek", SecretString::from(String::from("sk-stale")))
            .unwrap();
        let os = MemoryStore::default();
        os.set_key("deepseek", SecretString::from(String::from("sk-os")))
            .unwrap();
        let vault = CredentialVault::with_backends(Box::new(os), file);
        assert_eq!(
            vault.get_key("deepseek").unwrap().unwrap().expose_secret(),
            "sk-stale"
        );
        assert!(
            dir.join("credentials.json").exists(),
            "读取不以另一后端的旧值删除已确认文件条目"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vault_clear_fails_loudly_when_any_backend_fails() {
        // 任一后端清除失败都报错：调用方不能认为已清除。
        let dir = temp_test_dir("vault-clear");
        let file = CredentialFile::new(&dir);
        file.set_key("deepseek", SecretString::from(String::from("sk-x")))
            .unwrap();
        let vault = CredentialVault::with_backends(Box::new(FailingBackend), file);
        assert!(vault.clear_key("deepseek").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 永远失败的 OS 后端：模拟凭据服务不可用（无 DBus / 交互 Keychain 等）。
    struct FailingBackend;

    impl CredentialStore for FailingBackend {
        fn set_key(&self, _provider_id: &str, _secret: SecretString) -> Result<()> {
            Err(keyring_error(keyring::Error::PlatformFailure(Box::new(
                std::io::Error::other("no credential service"),
            ))))
        }

        fn get_key(&self, _provider_id: &str) -> Result<Option<SecretString>> {
            Err(keyring_error(keyring::Error::PlatformFailure(Box::new(
                std::io::Error::other("no credential service"),
            ))))
        }

        fn clear_key(&self, _provider_id: &str) -> Result<()> {
            Err(keyring_error(keyring::Error::PlatformFailure(Box::new(
                std::io::Error::other("no credential service"),
            ))))
        }
    }

    /// Windows CI：真实 Credential Manager 可用，直测 OS 库读写清除。
    /// macOS / Linux CI 无凭据会话服务（无 DBus / 交互 Keychain），跳过——
    /// 这两个平台的 OS 库路径在本仓标记为「未验证，不宣称支持」。
    #[cfg(windows)]
    #[test]
    fn os_keyring_round_trip_on_windows() {
        let store = OsKeyring::new();
        let id = "mythos-test-roundtrip";
        store
            .set_key(id, SecretString::from(String::from("sk-test-1234")))
            .unwrap();
        assert_eq!(
            store.get_key(id).unwrap().unwrap().expose_secret(),
            "sk-test-1234"
        );
        assert_eq!(
            store.status(id).unwrap(),
            KeyStatus {
                set: true,
                hint: Some("1234".into())
            }
        );
        store.clear_key(id).unwrap();
        assert!(store.get_key(id).unwrap().is_none());
    }

    #[test]
    fn secret_debug_never_contains_plaintext() {
        let secret = SecretString::from(String::from("sk-plaintext-marker"));
        let formatted = format!("{secret:?}");
        assert!(!formatted.contains("sk-plaintext-marker"));
        let status = KeyStatus {
            set: true,
            hint: Some("7890".into()),
        };
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains("sk-plaintext-marker"));
    }

    #[test]
    fn corrupt_credentials_file_maps_to_corrupt() {
        let dir = temp_test_dir("corrupt");
        std::fs::write(dir.join("credentials.json"), b"not json").unwrap();
        let store = CredentialFile::new(&dir);
        assert_eq!(store.get_key("deepseek").unwrap_err().code(), "corrupt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn defensive_json_mappers_map_to_corrupt() {
        // 防御性映射正常不可达（受控类型不会序列化失败），按映射器直测约定覆盖。
        let json_error = || serde_json::from_str::<()>("garbage").unwrap_err();
        assert_eq!(err_credentials_json(json_error()).code(), "corrupt");
        assert_eq!(err_credentials_serialize(json_error()).code(), "corrupt");
    }
}
