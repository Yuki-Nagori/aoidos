// Windows release 构建下不显示额外的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    mythos_lib::run();
}
