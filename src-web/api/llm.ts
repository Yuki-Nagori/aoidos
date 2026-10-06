import { invoke } from "@tauri-apps/api/core";

/** 对应 Rust Sampling；temperature 与 thinking 能力冲突时由 Rust 侧省略参数。 */
export interface Sampling {
  temperature: number;
  maxTokens: number;
}

/** 调用形态；叙事固定 completion，chat 留给非叙事调用。 */
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

/** 查询凭据状态（只返回 set + hint，不读取明文）。 */
export function getKeyStatus(providerId: string): Promise<KeyStatus> {
  return invoke("llm_get_key_status", { providerId });
}

/**
 * 设置 / 清除密钥。action="set" 发起 Rust 原生输入（Windows CredUI，
 * 未验证平台拒绝），用户取消保留旧值；明文不接受 IPC 参数。
 */
export function setKey(providerId: string, action: "set" | "clear"): Promise<KeyStatus> {
  return invoke("llm_set_key", { providerId, action });
}
