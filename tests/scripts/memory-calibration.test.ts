import { describe, expect, it } from "vitest";
import { validateDataset } from "../../scripts/memory-calibration";

const categories = [
  "negation-correction",
  "same-name",
  "paraphrase-or-new-event",
  "unauthorized-character",
  "false-preference",
  "history-rewind",
  "cycle",
  "commitment",
  "relationship-change",
  "unresolved-clue",
  "ordinary-detail",
  "ambiguous-reference",
];

function dataset() {
  return {
    version: 1,
    datasetId: "memory-calibration-v1",
    groups: Array.from({ length: 60 }, (_, index) => {
      const groupId = `event-group-${index.toString().padStart(2, "0")}`;
      const sourceId = `source-event-${index.toString().padStart(2, "0")}`;
      return {
        groupId,
        split: index < 42 ? "tuning" : "holdout",
        categories: [categories[index % categories.length]!],
        sources: [{ sourceId, kind: "player-input", text: `原始事件 ${index}` }],
        expected: {
          decision: "remember",
          sourceIds: [sourceId],
          visibleSubjectIds: ["player"],
          requiredClaims: [`claim-${index}`],
          forbiddenClaims: [],
          allowedSummary: [`允许摘要 ${index}`],
          rationale: `标注理由 ${index}`,
        },
      };
    }),
  };
}

describe("memory calibration dataset validation", () => {
  it("accepts versioned original event groups with an exact group-level 70/30 split", () => {
    expect(validateDataset(dataset())).toEqual([]);
  });

  it("rejects fewer than 60 groups and non-70/30 split counts", () => {
    const short = dataset();
    short.groups.pop();
    expect(validateDataset(short)).toContain(
      "事件组总数必须不少于 60 且为 10 的倍数，才能精确按 70/30 划分",
    );

    const skewed = dataset();
    skewed.groups[0]!.split = "holdout";
    expect(validateDataset(skewed)).toContain("tuning 必须占 70%；当前 41/60");
  });

  it("rejects an exact source-text duplicate assigned across the two splits", () => {
    const value = dataset();
    value.groups[59]!.sources[0]!.text = value.groups[0]!.sources[0]!.text;
    expect(validateDataset(value)).toContain(
      "groups[59].sources[0].text 与另一数据集分区存在完全重复原文",
    );
  });

  it("requires all risk categories and keeps annotation sources inside their event group", () => {
    const value = dataset();
    for (const group of value.groups) group.categories = ["ordinary-detail"];
    value.groups[0]!.categories = ["not-registered"];
    value.groups[1]!.expected.sourceIds = ["source-event-02"];
    const issues = validateDataset(value);
    expect(issues).toContain("groups[0] 含未知类别 not-registered");
    expect(issues).toContain("缺少必测类别 negation-correction");
    expect(issues).toContain("groups[1].expected.sourceIds 引用了本组以外的来源");
  });

  it("requires every risk category in both tuning and holdout splits", () => {
    const value = dataset();
    value.groups[42]!.categories = ["ordinary-detail"];
    value.groups[54]!.categories = ["ordinary-detail"];

    expect(validateDataset(value)).toContain("holdout 缺少必测类别 cycle");
  });

  it("rejects missing rationale, unregistered sources and unknown outcomes", () => {
    const value = dataset();
    value.groups[0]!.expected.decision = "maybe";
    value.groups[0]!.expected.rationale = " ";
    value.groups[0]!.sources[0]!.kind = "provider-output";
    const issues = validateDataset(value);
    expect(issues).toContain("groups[0].expected.decision 无效");
    expect(issues).toContain("groups[0].expected.rationale 不能为空");
    expect(issues).toContain("groups[0].sources[0].kind 未登记");
  });

  it("rejects non-string identifiers without coercing malformed values", () => {
    const value = dataset();
    value.groups[0]!.groupId = Symbol("event-group") as unknown as string;
    value.groups[1]!.sources[0]!.sourceId = { toString: null } as unknown as string;

    expect(() => validateDataset(value)).not.toThrow();
    const issues = validateDataset(value);
    expect(issues).toContain("groups[0].groupId 格式无效");
    expect(issues).toContain("groups[1].sources[0].sourceId 格式无效");
  });

  it("requires positive labels to define source, visibility and summary boundaries", () => {
    const value = dataset();
    const expected = value.groups[0]!.expected;
    expected.sourceIds = [];
    expected.visibleSubjectIds = [];
    expected.allowedSummary = [];

    const issues = validateDataset(value);
    expect(issues).toContain("groups[0].expected.sourceIds：remember 必须引用至少一个来源");
    expect(issues).toContain("groups[0].expected.visibleSubjectIds：remember 必须指定可见主体");
    expect(issues).toContain("groups[0].expected.allowedSummary：remember 必须指定允许摘要");

    expected.visibleSubjectIds = [" "];
    expected.allowedSummary = [""];
    expect(validateDataset(value)).toContain("groups[0].expected.visibleSubjectIds 不得包含空白项");
    expect(validateDataset(value)).toContain("groups[0].expected.allowedSummary 不得包含空白项");
  });
});
