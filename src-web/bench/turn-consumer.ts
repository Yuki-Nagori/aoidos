// 正文事件校验基准；数值只供本机比较，不作为性能验收结论。
import { Bench } from "tinybench";
import { validTurnEvent, type TurnNotification } from "../utils/turn-consumer";
const event: TurnNotification = {
  kind: "chunk",
  envelope: { seq: 1, data: { turnId: "benchmark", delta: "雾钟地窖的钟声响起。" } },
};

const bench = new Bench({ warmupIterations: 100, iterations: 1_000 });
bench.add("validTurnEvent", () => validTurnEvent(event));

await bench.run();
console.table(bench.table());

// 门禁断言（verify 的 bench 项）：每个任务到达 completed 状态，否则非零退出；
// 基准数值本身不做阈值判定。tinybench 6 以 state（completed / errored）表达
// 成败，error 只存在于 errored 变体。
const failed = bench.tasks.filter((task) => !task.result || task.result.state !== "completed");
if (bench.tasks.length === 0 || failed.length > 0) {
  console.error(`bench 产物校验失败：${failed.length} 个任务异常 / 共 ${bench.tasks.length} 个`);
  process.exit(1);
}
