import { describe, expect, it } from "vitest";
import en from "./en.json";
import zhTW from "./zh-TW.json";
import zh from "./zh.json";

describe("first-run restore copy", () => {
  it.each([
    {
      locale: "English",
      copy: en.firstRun,
      centralLibrary: "central library",
      sourceRepository: "local source repositories",
      projectChain: "project skill chains",
      backup: "Patchbay backup",
      continueWithoutRestore: "without restoring",
    },
    {
      locale: "Simplified Chinese",
      copy: zh.firstRun,
      centralLibrary: "中央库",
      sourceRepository: "本机原件仓库",
      projectChain: "项目技能链",
      backup: "Patchbay 备份",
      continueWithoutRestore: "暂不恢复",
    },
    {
      locale: "Traditional Chinese",
      copy: zhTW.firstRun,
      centralLibrary: "中央庫",
      sourceRepository: "本機原件倉庫",
      projectChain: "專案技能鏈",
      backup: "Patchbay 備份",
      continueWithoutRestore: "暫不還原",
    },
  ])("distinguishes the central library from local skill chains in $locale", ({
    copy,
    centralLibrary,
    sourceRepository,
    projectChain,
    backup,
    continueWithoutRestore,
  }) => {
    expect(copy.title).toContain(centralLibrary);
    expect(copy.subtitle).toContain(sourceRepository);
    expect(copy.subtitle).toContain(projectChain);
    expect(copy.subtitle).toContain(backup);
    expect(copy.urlLabel).toContain(backup);
    expect(copy.startFresh).toContain(continueWithoutRestore);
    expect(copy.restore).toContain(centralLibrary);
  });
});
