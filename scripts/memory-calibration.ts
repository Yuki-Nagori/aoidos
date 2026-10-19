import { readFile } from "node:fs/promises";
import { argv, exit } from "node:process";

const categories = new Set([
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
]);
const sourceKinds = new Set([
  "player-input",
  "narration",
  "character-speech",
  "system-fact",
  "scene-event",
]);
const decisions = new Set(["remember", "not-worthy", "ask"]);

type Source = { sourceId: string; kind: string; text: string };
type Group = {
  groupId: string;
  split: "tuning" | "holdout";
  categories: string[];
  sources: Source[];
  expected: {
    decision: string;
    sourceIds: string[];
    visibleSubjectIds: string[];
    requiredClaims: string[];
    forbiddenClaims: string[];
    allowedSummary: string[];
    rationale: string;
  };
};
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function strings(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isValidId(value: unknown): value is string {
  return typeof value === "string" && /^[a-z0-9][a-z0-9-]{2,63}$/.test(value);
}

function validateDataset(value: unknown): string[] {
  const issues: string[] = [];
  if (!isObject(value)) return ["根节点必须是对象"];
  if (value.version !== 1) issues.push("version 必须为 1");
  if (value.datasetId !== "memory-calibration-v1") {
    issues.push('datasetId 必须为 "memory-calibration-v1"');
  }
  if (!Array.isArray(value.groups)) return [...issues, "groups 必须是数组"];

  const groups = value.groups;
  if (groups.length < 60 || groups.length % 10 !== 0) {
    issues.push("事件组总数必须不少于 60 且为 10 的倍数，才能精确按 70/30 划分");
  }

  const groupIds = new Set<string>();
  const sourceIds = new Set<string>();
  const splitByText = new Map<string, string>();
  const presentCategories = new Set<string>();
  const categoriesBySplit = new Map<"tuning" | "holdout", Set<string>>([
    ["tuning", new Set()],
    ["holdout", new Set()],
  ]);
  let tuningCount = 0;
  let holdoutCount = 0;

  for (const [index, rawGroup] of groups.entries()) {
    const path = `groups[${index}]`;
    if (!isObject(rawGroup)) {
      issues.push(`${path} 必须是对象`);
      continue;
    }
    const group = rawGroup as unknown as Group;
    if (!isValidId(group.groupId)) {
      issues.push(`${path}.groupId 格式无效`);
    } else if (groupIds.has(group.groupId)) {
      issues.push(`${path}.groupId 重复`);
    } else {
      groupIds.add(group.groupId);
    }

    if (group.split === "tuning") tuningCount += 1;
    else if (group.split === "holdout") holdoutCount += 1;
    else issues.push(`${path}.split 必须为 tuning 或 holdout`);

    if (!strings(group.categories) || group.categories.length === 0) {
      issues.push(`${path}.categories 必须是非空标签数组`);
    } else {
      for (const category of group.categories) {
        if (!categories.has(category)) issues.push(`${path} 含未知类别 ${category}`);
        presentCategories.add(category);
        if (group.split === "tuning" || group.split === "holdout") {
          categoriesBySplit.get(group.split)?.add(category);
        }
      }
    }

    if (!Array.isArray(group.sources) || group.sources.length === 0) {
      issues.push(`${path}.sources 必须包含原始事件`);
      continue;
    }

    const localSourceIds = new Set<string>();
    for (const [sourceIndex, rawSource] of group.sources.entries()) {
      const sourcePath = `${path}.sources[${sourceIndex}]`;
      if (!isObject(rawSource)) {
        issues.push(`${sourcePath} 必须是对象`);
        continue;
      }
      const source = rawSource as unknown as Source;
      if (!isValidId(source.sourceId)) {
        issues.push(`${sourcePath}.sourceId 格式无效`);
      } else if (sourceIds.has(source.sourceId) || localSourceIds.has(source.sourceId)) {
        issues.push(`${sourcePath}.sourceId 重复`);
      }
      sourceIds.add(source.sourceId);
      localSourceIds.add(source.sourceId);

      if (!sourceKinds.has(source.kind)) issues.push(`${sourcePath}.kind 未登记`);
      if (typeof source.text !== "string" || source.text.trim().length === 0) {
        issues.push(`${sourcePath}.text 必须保留非空原文`);
        continue;
      }
      const normalizedText = source.text.normalize("NFC").replace(/\s+/gu, " ").trim();
      const previousSplit = splitByText.get(normalizedText);
      if (previousSplit && previousSplit !== group.split) {
        issues.push(`${sourcePath}.text 与另一数据集分区存在完全重复原文`);
      }
      splitByText.set(normalizedText, group.split);
    }

    if (!isObject(group.expected)) {
      issues.push(`${path}.expected 必须包含人工标注`);
      continue;
    }
    const expected = group.expected;
    if (!decisions.has(expected.decision)) issues.push(`${path}.expected.decision 无效`);
    for (const field of [
      "sourceIds",
      "visibleSubjectIds",
      "requiredClaims",
      "forbiddenClaims",
      "allowedSummary",
    ] as const) {
      if (!strings(expected[field])) {
        issues.push(`${path}.expected.${field} 必须是字符串数组`);
      } else if (expected[field].some((item) => item.trim().length === 0)) {
        issues.push(`${path}.expected.${field} 不得包含空白项`);
      }
    }
    if (typeof expected.rationale !== "string" || expected.rationale.trim().length === 0) {
      issues.push(`${path}.expected.rationale 不能为空`);
    }
    if (strings(expected.sourceIds)) {
      if (expected.decision === "remember" && expected.sourceIds.length === 0) {
        issues.push(`${path}.expected.sourceIds：remember 必须引用至少一个来源`);
      }
      for (const sourceId of expected.sourceIds) {
        if (!localSourceIds.has(sourceId)) {
          issues.push(`${path}.expected.sourceIds 引用了本组以外的来源`);
        }
      }
    }
    if (
      expected.decision === "remember" &&
      strings(expected.visibleSubjectIds) &&
      expected.visibleSubjectIds.length === 0
    ) {
      issues.push(`${path}.expected.visibleSubjectIds：remember 必须指定可见主体`);
    }
    if (
      expected.decision === "remember" &&
      strings(expected.allowedSummary) &&
      expected.allowedSummary.length === 0
    ) {
      issues.push(`${path}.expected.allowedSummary：remember 必须指定允许摘要`);
    }
  }

  if (groups.length >= 60 && groups.length % 10 === 0) {
    if (tuningCount * 10 !== groups.length * 7) {
      issues.push(`tuning 必须占 70%；当前 ${tuningCount}/${groups.length}`);
    }
    if (holdoutCount * 10 !== groups.length * 3) {
      issues.push(`holdout 必须占 30%；当前 ${holdoutCount}/${groups.length}`);
    }
  }
  for (const category of categories) {
    if (!presentCategories.has(category)) issues.push(`缺少必测类别 ${category}`);
    for (const split of ["tuning", "holdout"] as const) {
      if (!categoriesBySplit.get(split)?.has(category)) {
        issues.push(`${split} 缺少必测类别 ${category}`);
      }
    }
  }
  return issues;
}

if ((import.meta as ImportMeta & { main?: boolean }).main === true) {
  const file = argv[2];
  if (!file || argv.length !== 3) {
    console.error("用法：bun scripts/memory-calibration.ts <数据集.json>");
    exit(2);
  }
  try {
    const dataset: unknown = JSON.parse(await readFile(file, "utf8"));
    const issues = validateDataset(dataset);
    if (issues.length > 0) {
      for (const issue of issues) console.error(`- ${issue}`);
      exit(1);
    }
    console.log(`记忆标注集格式通过：${file}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : "无法读取标注集");
    exit(1);
  }
}

export { validateDataset };
