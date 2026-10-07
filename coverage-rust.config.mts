// coverage:rust 覆盖率门禁配置，由 scripts/coverage-rust.mts 消费。
//
// llvm-cov 对「同一 crate 同时作为其他测试二进制的依赖被重复插桩」做跨二进制
// 合并时，会对内联副本与 serde derive 派生产生少量幽灵未覆盖行——`llvm-cov show`
// 显示全覆盖而 `llvm-cov report` 计 miss，无法经测试触达（实际额度以本文件台账为准）。
// 因此行覆盖门槛不设全局百分比，而是逐文件声明未覆盖行预算：
// 缺省 0（所有文件默认必须 100%），确属工具伪影的文件在此登记理由与额度；
// 真实回归会推高某文件的未覆盖数，照样拦截。

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
  ],
  /** 缺省每文件未覆盖行上限；0 表示必须 100%。 */
  defaultMaxUncoveredLines: 0,
  /**
   * 逐文件预算：键为 llvm-cov 路径的后缀（正斜杠归一后匹配），
   * 额度为该文件允许的未覆盖行绝对数。
   */
  allowances: [
    // 测试收尾语句（关停夹具 / 清理临时目录）的跨二进制归并伪影（019 实测）。
    { file: "src-rust/mythos-llm/src/proxy.rs", maxUncoveredLines: 1 },
    { file: "src-tauri/src/llm_commands.rs", maxUncoveredLines: 1 },
  ],
};
