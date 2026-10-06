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
use serde::Serialize;

use mythos_store::atomic::write_text_atomic;
use mythos_store::error::{Result, StoreError};

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
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 4 {
        None
    } else {
        Some(chars[chars.len() - 4..].iter().collect())
    }
}

/// OS 凭据库（keyring 默认特性，见 tech-stack）。service 固定为应用名，user 为 providerId。
pub struct OsKeyring {
    service: &'static str,
}

impl OsKeyring {
    #[must_use]
    pub fn new() -> Self {
        Self { service: "mythos" }
    }

    fn entry(&self, provider_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(self.service, provider_id).map_err(keyring_error)
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
/// [`write_text_atomic`] 覆写；文件权限在创建时收紧为仅当前用户（Unix 0600 /
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

    fn read_map(&self) -> Result<BTreeMap<String, String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(err_credentials_json),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(StoreError::from_io(err)),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> Result<()> {
        // parent 为 None 仅在路径是根时出现，落盘会在写入时报错，这里统一建目录。
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))
            .map_err(StoreError::from_io)?;
        let text = serde_json::to_string(map).map_err(err_credentials_serialize)?;
        // 先写后收紧：原子写完成发布，再压权限；中途崩溃最坏留下默认权限文件，
        // 下一次写入会重新收紧。密码正文永不经过日志。
        write_text_atomic(&self.path, &text)?;
        crate::platform::restrict_to_current_user(&self.path)
    }
}

impl CredentialStore for CredentialFile {
    fn set_key(&self, provider_id: &str, secret: SecretString) -> Result<()> {
        let mut map = self.read_map()?;
        map.insert(provider_id.to_owned(), secret.expose_secret().to_owned());
        self.write_map(&map)
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        Ok(self
            .read_map()?
            .get(provider_id)
            .map(|raw| SecretString::from(raw.clone())))
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        let mut map = self.read_map()?;
        map.remove(provider_id);
        if map.is_empty() {
            clear_file(&self.path)
        } else {
            self.write_map(&map)
        }
    }
}

/// 删除降级文件：缺失视为已删除（幂等）；其余失败如实上报。
fn clear_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(StoreError::from_io(err)),
    }
}

/// 优先 OS 凭据库、失败降级私有文件的组合存储。
///
/// 语义：写操作先尝试 OS 库，成功后清除降级文件里的同名旧副本（单向迁移，
/// OS 库恢复可用后不残留明文副本）；OS 库不可用时写降级文件。读操作先 OS 库，
/// 未命中或读取失败再查降级文件（OS 库瞬断不能把已降级保存的密钥读丢）；
/// OS 读命中时顺带重试迁移清理，把「清理失败 + OS 瞬断」期间读到过期副本的
/// 窗口压到最短。
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
            Ok(()) => {
                // 迁移清理：文件副本若存在则删除，失败不回滚 OS 库写入
                //（下次写入会再试清理，删除失败不构成密钥丢失）。
                let _ = self.file.clear_key(provider_id);
                Ok(())
            }
            Err(_) => self.file.set_key(provider_id, secret),
        }
    }

    fn get_key(&self, provider_id: &str) -> Result<Option<SecretString>> {
        match self.os.get_key(provider_id) {
            Ok(Some(secret)) => {
                // OS 库确认持有该密钥：顺带重试此前可能失败的迁移清理。
                // 只在 OS 读成功时清——OS 写失败过的 provider（文件是唯一
                // 副本）走 Ok(None) / Err 分支，不会被误删。
                let _ = self.file.clear_key(provider_id);
                Ok(Some(secret))
            }
            // 未命中或读取失败都回落降级文件（语义见 struct 文档）。
            Ok(None) | Err(_) => self.file.get_key(provider_id),
        }
    }

    fn clear_key(&self, provider_id: &str) -> Result<()> {
        // 两边都清：任一边失败即报错（清除未确认完整，OS 库可能仍持有同名
        // 条目）；文件副本的清除已在报错前尽力执行。
        let os_result = self.os.clear_key(provider_id);
        let file_result = self.file.clear_key(provider_id);
        os_result?;
        file_result
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
        assert!(!dir.join("credentials.json").exists());
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
            !dir.join("credentials.json").exists(),
            "OS 库可用后不得残留明文文件副本"
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
    fn vault_retries_pending_cleanup_on_successful_os_read() {
        // 上次「OS 写成功 + 文件清理失败」遗留的过期副本：OS 读命中时顺带
        // 重试清理，收窄「OS 瞬断读到过期副本」的时间窗。
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
            "sk-os"
        );
        assert!(
            !dir.join("credentials.json").exists(),
            "OS 读成功必须重试清理过期副本"
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
