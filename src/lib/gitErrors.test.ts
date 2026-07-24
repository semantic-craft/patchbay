import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import { mapGitErrorMessage } from "./gitErrors";

const echoKey = ((key: string) => key) as TFunction;

describe("mapGitErrorMessage", () => {
  it("preserves the GitHub App reauthorization recovery path", () => {
    expect(
      mapGitErrorMessage(
        new Error("GITHUB_APP_REAUTH_REQUIRED: authorization expired"),
        echoKey,
      ),
    ).toBe("backup.github.errorAppReauth");
  });

  it.each([
    ["GITHUB_APP_REPO_NOT_PRIVATE", "backup.github.errorAppRepoNotPrivate"],
    ["GITHUB_APP_INSTALLATION_SCOPE", "backup.github.errorAppInstallationScope"],
  ])("preserves the GitHub App repository safety path for %s", (code, key) => {
    expect(mapGitErrorMessage(new Error(`${code}: changed`), echoKey)).toBe(key);
  });
});
