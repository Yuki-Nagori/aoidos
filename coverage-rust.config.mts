// coverage:rust 覆盖率门禁配置，由 scripts/coverage-rust.mts 消费。
//
// 缺省每文件未覆盖行 = 0。平台 / 装配排除见 ignore；逐文件预算目前为空。
// LLVM 对函数实例组取已覆盖行数的最大值，并非各实例覆盖行的并集：
// 不同闭包 / 泛型实例分别覆盖成功与失败时，文件 segments 看似全绿而 summary
// 仍可能缺行。先核实 functions 并补同一实例的边界测试，不直接登记工具伪影。
// 037 已清除原有六处预算，依据与验证见 ai-docs/task/037-rust-coverage-audit.md。

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
  allowances: [] as { file: string; maxUncoveredLines: number }[],
};
