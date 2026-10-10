import { afterEach, describe, expect, it } from "vitest";
import { i18n } from "../../../src-web/i18n";
import { billingErrorDescriptor } from "../../../src-web/utils/billing-error";

function messageFor(code: string, message: string): string {
  const descriptor = billingErrorDescriptor({ code, message });
  return "values" in descriptor
    ? i18n.global.t(descriptor.key, descriptor.values)
    : i18n.global.t(descriptor.key);
}

describe("billing error localization", () => {
  afterEach(() => {
    i18n.global.locale.value = "en";
  });

  it("uses the selected locale instead of backend error prose", () => {
    i18n.global.locale.value = "en";
    expect(messageFor("budget.price-history-limit", "该模型价格版本已达到安全上限")).toBe(
      "This model has reached the maximum number of saved price versions.",
    );

    i18n.global.locale.value = "zh-Hans";
    expect(messageFor("budget.price-history-limit", "backend message")).toBe(
      "该模型已达到可保存的价格版本上限。",
    );
  });

  it("maps every stable application and billing fault code", () => {
    const cases = [
      ["app.not-found", "errors.appNotFound"],
      ["app.not-ready", "errors.appNotReady"],
      ["app.bad-request", "errors.badRequest"],
      ["app.busy", "errors.appBusy"],
      ["store.corrupt", "errors.storeCorrupt"],
      ["store.database", "errors.storeIo"],
      ["store.io", "errors.storeIo"],
      ["budget.conflict", "billing.errors.conflict"],
      ["budget.exceeded", "billing.errors.exceeded"],
      ["budget.price-history-limit", "billing.errors.priceHistoryLimit"],
      ["budget.price-missing", "billing.errors.priceMissing"],
      ["budget.run-limit-missing", "billing.errors.runLimitMissing"],
    ] as const;

    for (const [code, key] of cases) {
      expect(billingErrorDescriptor({ code, message: "backend prose" })).toEqual({ key });
    }
  });

  it("localizes common billing errors and sanitizes unknown codes", () => {
    i18n.global.locale.value = "en";
    expect(messageFor("app.bad-request", "金额或价格参数不合法")).toBe("The request is invalid.");
    expect(messageFor("budget.future-code", "backend message")).toBe(
      "Something went wrong (budget.future-code).",
    );
    expect(messageFor("", "raw backend message")).toBe(
      "The billing operation could not be completed.",
    );
  });
});
