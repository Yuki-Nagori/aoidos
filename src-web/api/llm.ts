import { invoke } from "@tauri-apps/api/core";

/** 对应 Rust Sampling；temperature 与 thinking 能力冲突时由 Rust 侧省略参数。 */
export interface Sampling {
  temperature: number;
  maxTokens: number;
}

/** 调用形态；能力与正式降级由 Rust 提交层检查。 */
export type ProfileMode = "completion" | "chat";

/**
 * 代理配置，三模式互斥。manual 的认证经 authRef 指向 OS 凭据库条目
 * （用户名 / 密码合并存储），明文不进配置或 IPC。
 */
export type ProxyConfig =
  { mode: "system" } | { mode: "none" } | { mode: "manual"; url: string; authRef?: string };

/** 对应 Rust LlmProfile；可保存的用户配置，提交回合时冻结。 */
export interface LlmProfile {
  profileId: string;
  providerId: string;
  model: string;
  mode: ProfileMode;
  thinking: boolean;
  sampling: Sampling;
  proxy: ProxyConfig;
}

/** 对应 Rust KeyStatus；明文密钥永不进前端，hint 为末 4 字符（短于 4 为 null）。 */
export interface KeyStatus {
  set: boolean;
  hint: string | null;
}

/** 读取全部 profile；硬上限 50 个，目录缺失为空列表。 */
export function listProfiles(): Promise<{ items: LlmProfile[] }> {
  return invoke("llm_list_profiles");
}

/** 保存单个 profile（同 id 覆盖）；结构非法或超上限拒绝 app.bad-request。 */
export function saveProfile(profile: LlmProfile): Promise<LlmProfile> {
  return invoke("llm_save_profile", { profile });
}

/** 删除 profile；不存在时也返回成功（幂等）。 */
export function deleteProfile(profileId: string): Promise<{ deleted: boolean }> {
  return invoke("llm_delete_profile", { profileId });
}

/** 查询凭据状态（Rust 派生 set + hint，明文不进入 IPC）。 */
export function getKeyStatus(providerId: string): Promise<KeyStatus> {
  return invoke("llm_get_key_status", { providerId });
}

/**
 * 设置 / 清除密钥。action="set" 发起 Rust 原生输入（Windows CredUI、macOS AppKit、Linux GTK），用户取消保留旧值；明文不接受 IPC 参数。
 */
export function setKey(providerId: string, action: "set" | "clear"): Promise<KeyStatus> {
  return invoke("llm_set_key", { providerId, action });
}

/** 已冻结配置的开发输入；生产构建不注册 llm_submit。 */
export type TurnInput =
  | { kind: "completion"; prompt: string }
  | {
      kind: "chat";
      messages: { role: "system" | "user" | "assistant"; content: string }[];
      assistantPrefix?: string;
    };

export type TurnOutcome = "completed" | "cancelled" | "failed";
export type FinishReason = "stop" | "guard" | "length";

/** 已提交正文、终态和三基线的一致副本；未发生的终态字段省略。 */
export interface TurnSnapshot {
  turnId: string;
  text: string;
  seq: { chunk: number; done: number; failed: number };
  outcome?: TurnOutcome;
  finishReason?: FinishReason;
  error?: { code: string; message: string };
}

/** 接纳后空快照已可读取；调试入口只运行 Rust 内建本地夹具。 */
export function submitTurn(
  profileId: string,
  input: TurnInput,
  guardSpecId: string,
): Promise<{ turnId: string }> {
  return invoke("llm_submit", { profileId, input, guardSpecId });
}

/** 读取内存回合；已驱逐的 UUID 返回 app.not-found。 */
export function getTurn(turnId: string): Promise<TurnSnapshot> {
  return invoke("llm_get_turn", { turnId });
}

/** 等待当前提交结束，返回取消与完成竞争后已胜出的终态。 */
export function cancelTurn(turnId: string): Promise<{ turnId: string; outcome: TurnOutcome }> {
  return invoke("llm_cancel", { turnId });
}

/** 事件的共同信封，各事件种类独立计数。 */
export interface TurnEnvelope<T> {
  seq: number;
  data: T;
}
export interface TurnChunk {
  turnId: string;
  delta: string;
}
export type TurnDone =
  | { turnId: string; outcome: "completed"; chunkSeq: number; finishReason: FinishReason }
  | { turnId: string; outcome: "cancelled"; chunkSeq: number };
/** empty-output 的 finishReason 必有，其余错误按真实已知原因携带。 */
export interface TurnFailed {
  turnId: string;
  code: string;
  message: string;
  chunkSeq: number;
  finishReason?: FinishReason;
}
