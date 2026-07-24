import { describe, expect, it } from "vitest";
import en from "./en.json";
import zhTW from "./zh-TW.json";
import zh from "./zh.json";

describe("Patchbay branding", () => {
  it("uses the shipped product name in every locale's status copy", () => {
    for (const locale of [en, zh, zhTW]) {
      expect(locale.settings.version).toMatch(/^Patchbay /);
      expect(locale.settings.panicBanner).toContain("Patchbay");
    }
  });
});
