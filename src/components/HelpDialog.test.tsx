import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../context/AppContext", () => ({
  useApp: () => ({ helpOpen: true, closeHelp: vi.fn() }),
}));

import { HelpDialog } from "./HelpDialog";

describe("HelpDialog", () => {
  it("leads with Patchbay's project-local three-tier model and Global Guard", () => {
    render(<HelpDialog />);

    expect(screen.getByRole("heading", { level: 2 }).textContent).toBe("Quick Start for Patchbay");
    expect(
      screen.getByText(
        "Start with the project-only chain: Original Repository → project aggregate (.agents/skills) → Agent entry. Global Guard keeps global Agent surfaces empty. Press ⌘K anytime for the command palette.",
      ).textContent,
    ).toContain("Global Guard");
    expect(screen.getByRole("heading", { level: 3, name: "Project-local three-tier workflow" })).toBeDefined();
    expect(screen.getByRole("heading", { level: 3, name: "Global Guard" })).toBeDefined();
  });
});
