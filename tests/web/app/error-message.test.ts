import { afterEach, describe, expect, it } from "vitest";
import { appErrorDescriptor } from "../../../src-web/app/error-message";
import { i18n } from "../../../src-web/i18n/index";

function messageFor(code: string, message: string, reason?: string): string {
  const descriptor = appErrorDescriptor({ code, message }, reason);
  return "values" in descriptor
    ? i18n.global.t(descriptor.key, descriptor.values)
    : i18n.global.t(descriptor.key);
}

describe("application error localization", () => {
  afterEach(() => {
    i18n.global.locale.value = "en";
  });

  it("localizes the billing budget error without exposing its diagnostic code", () => {
    i18n.global.locale.value = "zh-Hans";
    expect(messageFor("budget.exceeded", "本局对应币种额度不足")).toBe(
      "本局对应服务商计价币种的上限已达到。",
    );
    expect(messageFor("budget.exceeded", "backend prose")).not.toContain("budget.exceeded");
  });

  it("preserves specialized recovery messages and hides backend prose", () => {
    expect(messageFor("app.bad-request", "backend prose", "stalePresenceRevision")).toBe(
      "The cast changed. Choose a character again.",
    );
    expect(messageFor("app.event-failed", "backend prose")).toBe(
      "State synchronization failed. Reconnect to restore it.",
    );
  });
});
