// coverage:rust 门禁执行器：跑 llvm-cov（JSON 导出），按根目录
// coverage-rust.config.mts 的聚合行覆盖底线裁决。
import { exit } from "node:process";
import { cargoInvocation, rustToolchainEnv } from "./rust-toolchain";
import config from "../coverage-rust.config.mts";

const reportPath = "target/coverage-rust.json";
const ignoreRegex = config.ignore.map((pattern) => pattern.source).join("|");

const run = Bun.spawnSync(
  [
    "rustup",
    ...cargoInvocation([
      "llvm-cov",
      "--workspace",
      "--lib",
      "--json",
      `--ignore-filename-regex=${ignoreRegex}`,
      `--output-path=${reportPath}`,
    ]),
  ],
  { env: rustToolchainEnv(), stdout: "inherit", stderr: "inherit" },
);
if (!run.success) {
  exit(run.exitCode ?? 1);
}

const report = JSON.parse(await Bun.file(reportPath).text());
let totalLines = 0;
let coveredLines = 0;
for (const file of report.data[0].files) {
  const path = file.filename.replaceAll("\\", "/");
  if (config.ignore.some((pattern) => pattern.test(path))) {
    continue;
  }
  totalLines += file.summary.lines.count;
  coveredLines += file.summary.lines.covered;
}

if (totalLines === 0) {
  console.error("Rust 覆盖率报告没有纳入任何 workspace lib 源行。");
  exit(1);
}

const coveragePercent = (coveredLines / totalLines) * 100;
console.log(
  `workspace lib 覆盖 ${coveredLines}/${totalLines} 行 (${coveragePercent.toFixed(2)}%); 最低要求 ${config.minimumLineCoveragePercent}%。`,
);
if (coveragePercent < config.minimumLineCoveragePercent) {
  console.error("Rust workspace lib 行覆盖率低于最低要求。");
  exit(1);
}
