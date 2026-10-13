// coverage:rust 覆盖率门禁配置，由 scripts/coverage-rust.mts 消费。
//
// 覆盖率是漏测线索，不要求每个源文件逐行填满；关键行为由测试规范中的风险清单约束。
// 037 已清除原有逐文件豁免，依据见 ai-docs/task/037-rust-coverage-audit.md。

export default {
  /** 不进门禁的文件名正则（对 llvm-cov 路径做子串匹配）。 */
  ignore: [
    // lib.rs 是装配入口，只放模块声明 / 薄装配（testing.md 约定）。
    /lib\.rs$/,
    // platform/windows.rs：Win32 安全 API 的失败分支（进程令牌 SID 提取中的
    // 内存分配 / CopySid 失败、合法 SID 的空值防御）无法在健康进程注入，整文件
    // 出门槛；可注入行为仍直测（空 / 非法句柄、非法 SID、坏路径、DACL 结构、
    // CredUI 对话框取消路径与返回码映射），依据见 testing.md。
    /platform[\\/]windows\.rs$/,
    // 真实 AppKit / GTK 主线程原生 UI，由 test:native 桌面烟测验证；不含存储业务。
    /platform[\\/](macos|linux)\.rs$/,
    // Wry 菜单 / 托盘构造和主线程更新只能在真实桌面会话创建；对应双语表面由
    // native-platform 集成测试运行。命令协调、持久化、状态聚合仍在门禁内。
    /locale_commands[\\/]native\.rs$/,
  ],
  /** 工作区 lib 目标的最低行覆盖率；不按小文件逐一设门槛。 */
  minimumLineCoveragePercent: 90,
};
