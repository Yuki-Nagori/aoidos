//! 两字段 UI 偏好；只在持久化成功后确认，不保存焦点或输入草稿。

use crate::{
    fault::Fault,
    record::{facts::DiceMode, session::store_fault},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiPreferences {
    pub version: u32,
    pub panel_pinned: bool,
    pub dice_mode: DiceMode,
}
impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            panel_pinned: false,
            dice_mode: DiceMode::Manual,
        }
    }
}
pub struct Preferences {
    path: PathBuf,
    gate: Mutex<()>,
}
impl Preferences {
    pub fn new(root: &Path) -> Self {
        Self {
            path: root.join("ui-preferences.json"),
            gate: Mutex::new(()),
        }
    }
    /// 缺失文件使用首次默认，损坏 / 未来版本不能重置覆盖。
    /// # Errors
    /// 有界读取或 schema 校验失败返回 store.*。
    pub fn get(&self) -> Result<UiPreferences, Fault> {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.read()
    }
    fn read(&self) -> Result<UiPreferences, Fault> {
        let Some(text) =
            mythos_store::read::read_text_bounded(&self.path, 4096).map_err(store_fault)?
        else {
            return Ok(UiPreferences::default());
        };
        let preferences: UiPreferences = mythos_json::decode(&text, 4096).map_err(corrupt)?;
        if preferences.version != 1 {
            return Err(Fault::new("store.corrupt", "偏好版本不受支持"));
        }
        Ok(preferences)
    }
    /// 完整替换两个可写偏好；未知字段由命令参数解码拒绝。
    /// # Errors
    /// 保存前核验既有版本；原子发布失败不返回成功值。
    pub fn set(&self, panel_pinned: bool, dice_mode: DiceMode) -> Result<UiPreferences, Fault> {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _previous = self.read()?;
        let preferences = UiPreferences {
            version: 1,
            panel_pinned,
            dice_mode,
        };
        let bytes = mythos_json::to_vec(&preferences).map_err(corrupt)?;
        mythos_store::atomic::write_atomic(&self.path, &bytes).map_err(store_fault)?;
        Ok(preferences)
    }
}
fn corrupt(_: mythos_json::Error) -> Fault {
    Fault::new("store.corrupt", "偏好文件不合法")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_confirm_only_persisted_values_and_preserve_future_files() {
        let dir = std::env::temp_dir().join(format!("mythos-prefs-{}", uuid::Uuid::new_v4()));
        let service = Preferences::new(&dir);
        assert!(!service.get().unwrap().panel_pinned);
        assert!(!dir.exists());
        let saved = service.set(true, DiceMode::Auto).unwrap();
        assert!(saved.panel_pinned);
        assert!(service.get().unwrap().panel_pinned);
        std::fs::write(
            &service.path,
            "{\"version\":2,\"panelPinned\":false,\"diceMode\":\"manual\"}",
        )
        .unwrap();
        assert!(service.set(false, DiceMode::Manual).is_err());
        assert!(service.get().is_err());
        std::fs::write(&service.path, "bad").unwrap();
        assert!(service.get().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
