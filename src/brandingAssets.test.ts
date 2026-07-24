import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

describe("Patchbay branding assets", () => {
  it("uses the packaged app icon in the sidebar", () => {
    const sidebarIcon = readFileSync(join(process.cwd(), "public/icons/32x32.png"));
    const packagedIcon = readFileSync(join(process.cwd(), "src-tauri/icons/32x32.png"));

    expect(sidebarIcon.equals(packagedIcon)).toBe(true);
  });
});
