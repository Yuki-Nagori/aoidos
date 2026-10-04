//! 命令层：解参数、调逻辑、回包；逻辑文件计入覆盖率门禁（装配 lib.rs 不计）。

use crate::ipc::CmdError;

/// 示例命令：前端 `invoke("greet", { name })` 调用。首个真实命令落地时替换。
/// 契约要求所有命令统一 `Result<T, CmdError>` 形状；本命令当前无失败路径。
#[tauri::command]
pub fn greet(name: &str) -> Result<String, CmdError> {
    Ok(format!("Hello, {name}! You've been greeted from Rust!"))
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn greets_the_given_name() {
        assert_eq!(
            greet("Tauri").unwrap(),
            "Hello, Tauri! You've been greeted from Rust!"
        );
    }
}
