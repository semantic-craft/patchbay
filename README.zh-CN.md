<p align="center">
  <img src="assets/icon.png" width="80" />
</p>

<h1 align="center">Patchbay</h1>

<p align="center">
  给某个项目，从你的 Git 原件仓里挑技能挂上去。
</p>

<p align="center">
  <a href="./README.md">English</a>
</p>

## 三层链路

Patchbay 只做一件事：维护「原件仓 → 项目 → Agent 入口」这条链路。

```
① 技能源              ② 项目 .agents/skills        ③ Agent 入口
Git 原件仓  ─────────▶  项目白名单（软链）  ─────────▶  .claude/skills
（你的技能仓库）         （这个项目要哪些技能）        .codex/skills …
```

1. **技能源** — 你自己的 Git 仓库。Patchbay 只读它们，不复制、不托管、不改写。
2. **项目 `.agents/skills`** — 每个项目一份白名单，用软链指回原件仓。这里才决定"这个项目用哪些技能"。
3. **Agent 入口** — 各 Agent 的技能目录指向项目的聚合层，一处白名单对所有 Agent 生效。

配套的一条硬规则：**全局技能面必须为空**。`~/.claude/skills`、`~/.codex/skills` 之类的全局目录里出现任何技能，都会在主屏顶部报警——技能属于项目，不属于机器。

## 功能

- **项目工作台** — 主屏。选中项目即可看到它的三层状态、挂载与解绑技能、逐条查看链路解析。
- **链路总览** — 全部技能源、项目聚合层和 Agent 入口的连线图，标出直挂仓库（绕过聚合层）和断链。
- **诊断** — 按规则扫描链路与 instructions，给出证据、可预览的修复，以及可撤销的修复记录。
- **开发源** — 技能源仓库的 Git 健康（脏工作树、领先/落后上游）和重复检出识别。
- **Instructions 治理** — `CLAUDE.md` / `AGENTS.md` 的正本与入口状态、逐 Agent 常驻 token 成本，以及 `normalize` / `init` 的预览式修复。
- **多机** — 跨机器的仓库清单、状态矩阵与受控的 push / pull / bootstrap。
- **Preset** — 把一组常用技能存成套装，下次给新项目一键起步。

## 快速上手

1. 在 **设置 → 原始仓库根目录** 里指向你放技能仓库的目录（例如 `~/Projects/my-skills`）。
2. 侧边栏 **关联项目**，选中要接线的项目。
3. 在工作台点 **挂技能**，挑选技能并勾选要开的 Agent —— Patchbay 会建好 `.agents/skills` 白名单和各 Agent 入口。
4. 顶部横幅报警时，去 **诊断** 分区看证据并预览修复。

## 支持的 Agent

Cursor · Claude Code · Codex · Grok · OpenCode · Amp · Kilo Code · Roo Code · Goose · Gemini CLI · GitHub Copilot · Windsurf · TRAE IDE · Antigravity · Clawdbot · Droid

也可以在**设置**里添加自定义 Agent 并指定它的技能目录。

## 技术栈

| 层 | 技术 |
|----|------|
| 前端 | React 19、TypeScript、Vite、Tailwind CSS |
| 桌面 | Tauri 2 |
| 后端 | Rust |
| 存储 | SQLite（`rusqlite`），位于 `~/.patchbay` |
| 国际化 | react-i18next |

## 快速开始

### 前置依赖

- Node.js 20.19+、22.13+ 或 24+
- Rust 工具链
- 当前系统的 [Tauri 依赖](https://v2.tauri.app/start/prerequisites/)

### 开发

```bash
npm install
npm run tauri:dev
```

### CLI

`patchbay-cli` 与桌面应用共用同一个 Rust core 和同一个数据库，`--json` 输出的就是 GUI 消费的那份契约。

```bash
# 本机数据目录与数据库位置
npm run cli -- --json status

# 已检测到的 Agent 与其技能目录
npm run cli -- tools list

# 三层链路——扫描、诊断、登记 Doctor 裁决
npm run cli -- --json chain topology
npm run cli -- --json chain doctor
npm run cli -- --json chain decide --fingerprint <fp> --action ignore          # 只读预览
npm run cli -- --json chain decide --fingerprint <fp> --action ignore --apply

# instructions 治理（CLAUDE.md / AGENTS.md）
npm run cli -- instructions scan
npm run cli -- instructions where --project /path/to/proj --agent claude
npm run cli -- instructions doctor --severity warning --rule dual_body
npm run cli -- instructions normalize --project /path/to/proj                  # 预览修复 plan
npm run cli -- instructions normalize --project /path/to/proj --fingerprint <fp> --apply
npm run cli -- instructions init --project /path/to/proj --docs-dir --apply

# 多机：状态矩阵与受控同步
npm run cli -- --json fleet status
npm run cli -- --json fleet push --apply
```

命令分组：

- `status`：本机数据目录与数据库路径
- `tools`：已检测到的 Agent 目标与路径
- `chain`：检查并修复三层链路；`decide` 以 fingerprint 登记 Doctor 裁决（`--action mark-private|ignore`），默认只预览，只有 `--apply` 才写入
- `instructions`：`scan`（正本、逐 Agent 入口状态与常驻 token 成本）、`where`（逐 Agent 读链，含 import 跳数）、`doctor`（十四条治理规则，规则 id 稳定，支持 `--severity`/`--rule` 过滤与按 fingerprint 忽略）、`normalize`（preview→apply 机械修复，写前快照、正本永不改写）、`init`（preview→apply 骨架搭建，只创建、幂等）
- `fleet`：多机仓库清单、状态矩阵与 push / pull / bootstrap，写操作一律 preview→apply

`--json` 给脚本和 agent 使用，输出机器可读。

#### 把 CLI 二进制安装到 PATH

```bash
npm run cli:install
# 等价于：
# cargo install --path src-tauri --bin patchbay-cli --locked --force
```

二进制会装到 `~/.cargo/bin/patchbay-cli`。代码更新后再跑一次即可刷新。

#### 与桌面应用并发使用

CLI 和桌面应用共享同一个 SQLite 数据库。SQLite 会串行化写入，所以数据是安全的，但运行中的应用不会自动刷新内存缓存 —— CLI 跑完会改状态的命令后，在应用里点一次「重新扫描」。

### 应用更新

Patchbay 官方 macOS 版本会在启动后每天检查一次签名发布通道。新版本不会静默安装：只有在你点击"安装并重启"后，Patchbay 才会下载、校验、安装并重启。你可以在设置中关闭自动检查或手动检查更新；发布页下载仍作为兜底方式保留。

### 构建

```bash
npm run tauri:build
npm run cli:build
```

维护者请参阅 [RELEASING.md](RELEASING.md)，其中记录版本、签名、公证、更新器和公开发布门禁。

## 常见问题

### macOS 首次启动被 Gatekeeper 拦截

Patchbay 官方 macOS 发布包均采用 Developer ID 签名并通过 Apple 公证。来自 [Patchbay Releases](https://github.com/semantic-craft/patchbay-releases/releases) 的未修改下载应能通过 Gatekeeper。

如果官方包被拦截，请从该页面重新下载，并报告发布 tag、macOS 版本及 Gatekeeper 的完整提示。不要绕过 Gatekeeper，也不要清除官方发布包的隔离属性。本地源码构建属于开发产物，不保证带有 Apple 公证票据。

## License

MIT
