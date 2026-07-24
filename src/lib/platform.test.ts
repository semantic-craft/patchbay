import { describe, expect, it } from "vitest";
import { applyPlatformAttribute, detectPlatform } from "./platform";

const WINDOWS_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

describe("detectPlatform", () => {
  it("recognizes Windows, which draws its own caption bar", () => {
    expect(detectPlatform(WINDOWS_UA)).toBe("windows");
  });

  it("treats macOS as the overlay-titlebar platform", () => {
    expect(detectPlatform(MAC_UA)).toBe("other");
  });
});

describe("applyPlatformAttribute", () => {
  it("tags the root so CSS can collapse the drag strip", () => {
    const root = document.createElement("div");
    expect(applyPlatformAttribute(root, WINDOWS_UA)).toBe("windows");
    expect(root.dataset.platform).toBe("windows");
  });

  it("leaves the overlay titlebar in place elsewhere", () => {
    const root = document.createElement("div");
    applyPlatformAttribute(root, MAC_UA);
    expect(root.dataset.platform).toBe("other");
  });
});
