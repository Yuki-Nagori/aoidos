/** 保留安全错误字段；其他异常统一转换为调用方提供的固定提示。 */
export function safeError(
  error: unknown,
  fallbackMessage: string,
  fallbackCode = "app.event-failed",
): { code: string; message: string } {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof error.code === "string" &&
    typeof error.message === "string"
  ) {
    return { code: error.code, message: error.message };
  }

  return { code: fallbackCode, message: fallbackMessage };
}
