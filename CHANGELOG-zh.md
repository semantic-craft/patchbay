# 更新日志

本文记录 Patchbay 自首个独立版本以来的所有显著变更。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [1.32.1] - 2026-07-29

### 发布概览
- 集中修复 1.32.0 之后发现的 Fleet 清单编辑与同步问题，强化 Git 远端预检，并完成当前兼容范围内的依赖安全更新。

### 用户可见更新
- **Fleet 清单编辑完整可用。** 桌面端改用独立的复数 IPC 数据结构，同时保持既有单数 TOML 存储格式；新增、修改、移除均可完整经过预览与应用，过期计划会被拒绝，原有 `repos.map` 错误不再出现。
- **Fleet 同步能够从陈旧缓存拒绝中恢复。** push、pull、bootstrap 和 init 会在刷新清单后重新判断资格，不再重复返回与 hub 最新清单相矛盾的拒绝结果。
- **Git 同步更加安全。** origin URL 与 push URL 会在本地 fast-forward 之前完成校验；配置异常时，工作区不会被移动。

### 开发者与治理更新
- 将 `git2` 升级到 0.21，显式保留 HTTPS、SSH 与 vendored OpenSSL transport，并补齐跨平台回归验证。
- 刷新 Tauri 兼容依赖，移除旧 `rand 0.7` / selector 链和已撤回的 `num-bigint 0.4.7`；GTK3/glib 0.18 告警继续作为上游约束明确保留，不做不受支持的强制覆盖。
- 将 React Router 更新至 7.18.2、ESLint 工具链更新至 10.8.0，在兼容范围内消除目标安全依赖链，并通过 warning 上限保持既有 React Compiler 诊断可见。
- 发布准备流程改为通过受保护主分支的 bump PR；合并后自动创建版本 tag 并触发托管发布流水线。

## [1.32.0] - 2026-07-24

### 发布概览
- Patchbay 正式开源:仓库以 MIT 许可公开、历史为干净根。本版是第一个完全在 GitHub 托管 runner 上构建、签名、公证并发布的版本,发布链路不再依赖任何自建 runner。
- Patchbay 重新成为多平台应用:1.31.0 说明中「产品范围收敛为 macOS-only 后放弃 Windows 支持(#47)」已不再成立,#58 恢复了 Windows 支持。旧条目按当时决策原样保留,本节即为其反转。

### 面向用户
- **下载迁回主仓库**:发布产物改为发布在本仓库,应用的更新源地址随之变更。1.31.0 及更早版本指向旧地址,不会自动收到本次更新——请到 Releases 页手动下载一次 1.32.0,之后自动更新恢复正常。
- **Windows 版开始发布** —— `release.yml` 新增 `build-windows` 任务,产出 NSIS 安装包。安装包**未签名**,首次运行会出现 SmartScreen 提示;自动更新不受影响,因为更新器校验的是 minisign 签名而非 Authenticode。

### 开发者与治理
- Windows `cargo test` 从 631 通过 / 53 失败提升到 812 通过 / 0 失败:模块级 `#[cfg(all(test, unix))]` 门全部移除,软链夹具经共享 `core::test_support` 在 Windows 上以 junction 兜底运行,无需开发者模式。
- 移植暴露并修复的真实 Windows 缺陷:`resolve_hub_base` 误用 `is_absolute()`;`chain::ops::make_symlink` 缺 junction 兜底;六处 `remove_file` 无法删除目录软链;`AGENT_SURFACES` 手拼相对路径(已收敛到 `project_links::surface_path`);`patchbay-cli` 只读 `$HOME`;`Cargo.toml` 版本停在 1.0.0。
- `content_hash` 此前仅在 unix 折叠可执行位,同一技能跨系统哈希不同,混合机队对比会出现幻影差异;现统一折叠、默认「非可执行」,既有 macOS 哈希不变。
- AES-256-GCM 密钥文件在 Windows 上收紧权限(去除继承,仅 owner 与 SYSTEM),且每次加载时重新收紧,旧密钥自动自愈。
- `npm test`(含发布闸 `release-contract.test.ts`)改为在 PR 上运行,而非首次执行于 tag 已推送之后。
- CI 全部运行在 GitHub 托管 runner(`macos-14` / `windows-latest` / `ubuntu-latest`):每次 push/PR 跑测试与服务端密钥扫描(`security.yml`),Claude 工作流仅响应仓库 owner/member/collaborator。
- 测试夹具不再依赖运行机器:Windows 密钥文件夹具自行种入可继承 ACL,不再假设 TEMP 落点;fleet 夹具固定设备名,不再读取真实主机名。
- 依赖治理:升级存在漏洞的 npm 与 Rust 依赖;更新 GitHub Action 运行时。

### 已知空缺
- Windows 安装包有意保持未签名:签名需在 `tauri.windows.conf.json` 配置 `certificateThumbprint` 并在 runner 导入证书;半接线、静默失效的签名路径比可见的缺失更糟。
- `prompt-optimizer` 在 #47 P3 闸的往返只验证了一半:非权威机已从 hub 快进,但提交并推送一侧属于权威机,无法从非权威机驱动。

## [1.31.0] - 2026-07-18

### 发布概览
- 交付多机仓库同步 epic(#25):多台机器通过共享 hub 保持项目仓一致,每台机器只操作自己的工作副本。本版含 P0(只读状态)、P1(push/pull/bootstrap/init)与 P3(脚本收编);P2 自动 round 默认关闭,等观察窗口结束。

### 面向用户
- **多机页面** —— 仓库 × 机器的状态矩阵:每格显示 `branch@head`、未提交数量与相对 hub 的偏离。本机列实测,他机列回放各自最近一次上报并诚实标注相对时间,超过七天标为过期。
- **带守卫的同步动作** —— push(权威机 → hub)、pull(hub → 干净的非权威副本,仅快进)、bootstrap(克隆本机缺失的纳管仓)全部走 预览 → 确认 → 执行;遇到脏工作树、游离 HEAD、历史分叉或证据漂移时拒做而非猜测。
- **清单编辑器** —— 一键收编扫描到的仓库,编辑权威机/hub/分支,或将某仓移出纳管。移除只改清单,绝不动磁盘上的工作副本或镜像。
- **本机 hub 建仓与 remote 收敛** —— 在 hub 宿主机创建缺失的裸镜像,并收敛各机的 hub remote,`origin` 留给你自己的上游。
- **自动 round(默认关闭)** —— 需同时开启全局开关与单仓 opt-in 才会执行;干净的仓自行快进或推送,其余一律只报告不强制。

### 开发者与治理
- 新增 `core/fleet/`(清单、元数据仓、仓库操作、服务、自动 round)与 `patchbay-cli fleet status|discover|report|push|pull|bootstrap|init`。设计正本:`docs/xw-fleet-sync-design.md`。
- 仓库清单迁至 hub 侧元数据仓的 `manifest.toml`,终结了 the legacy per-machine config 两份副本互相覆盖的整类隐患。the sync script 由 747 行缩为 316 行薄壳,全部委派 CLI,缺 CLI 时 exit 127 并给出指引。
- 每个写动词都记录计划证据,并在 `fleet.lock` 内重新校验后才动手,任何漂移都转为逐项冲突。fleet 的任何动词都不能 merge、rebase、force、reset、stash、自动提交或删除。
- 安全:清单中的 hub URL 曾未经 `--` 分隔就传给 `git ls-remote`,导致以 `-` 开头的值被当作选项解析,`--upload-pack=<命令>` 会被执行——且发生在只读的状态路径上(#54)。已修复,并对 `hub.url` 增加传输协议白名单校验。
- epic 上记录的已接受偏差:权威机允许 bootstrap 自己的仓(它只写不存在的路径,保护事实源的规则在此没有保护对象)(#56);产品范围收敛为 macOS-only 后放弃 Windows 支持(#47)。

## [1.30.0] - 2026-07-18

### 发布概览
- 交付 Liquid Glass 工作台整轮（#26）：工作台成为主屏并获得原生玻璃窗口质感，链路维护改为异常驱动、修复可撤销，并支持 Preset 驱动的项目接入。

### 用户可见更新
- **Liquid Glass 工作台** — 项目工作台成为主路由，浅/深两套玻璃皮肤：壁纸底的窗口壳、玻璃侧栏与悬浮玻璃卡片。
- **macOS 窗口级玻璃** — 窗口转为透明：macOS 26 上启用真 Liquid Glass（NSGlassEffectView），旧版 macOS 回退磨砂 vibrancy，两者都不可用时自动退回不透明底——功能不减。
- **原生外观跟随主题** — 在设置中选择浅色/深色/跟随系统时，原生窗口材质与标题栏同步切换，玻璃不再与界面主题错配。
- **异常驱动的链路维护** — 链路健康时呈现安静的全绿屏；断链浮现为证据卡（候选路径、git 线索），通过确定性的 plan/apply 修复动线处理，支持步进直播与暂停/接管。
- **修复留痕与撤销** — 每次修复落账 journal 并记录逆操作，可一键受保护地撤销。
- **共因批量重链** — 仓库迁移导致的大面积断链聚合为一张共因卡，一键批量重链。
- **反哺提示卡** — 原件仓库存在未提交改动时浮现琥珀色提示卡。
- **Chain Preset 与接入向导** — 可把当前技能集合存为 Preset 并从 Preset 栏应用；零链接项目通过三步向导（选来源 → 挑技能 → 建入口）完成接入。

### 开发者与治理更新
- 窗口玻璃走三档运行时探测（`liquid-glass` / `vibrancy` / `none`），降级判定为纯函数并有单测覆盖；仅在确认原生材质就位后 CSS 壁纸才让位。
- 采用 `tauri-plugin-liquid-glass`（自带 cocoa/objc 绑定）规避 window-vibrancy 0.6/0.8 符号冲突（tauri#15478）；已接受偏差记录于 #37：非焦点窗口恒为雾面、`macOSPrivateApi` 不符 App Store 合规。
- Agent instructions 支持矩阵调研（issue #3）落入 `docs/research/`。

## [1.29.4] - 2026-07-16

### 发布概览
- 在整理后的 Patchbay 代码库上增加由用户确认的桌面应用自动更新，并完成剩余的仓库治理 tickets。

### 用户可见更新
- **自动检查签名更新** — 官方 macOS 版本会在启动后每天检查一次 Patchbay 公开发布通道，发现新版本时持续显示可操作提示。
- **安装仍由用户控制** — 只有在用户点击“安装并重启”后，Patchbay 才会下载、校验、安装并重启；自动检查可以关闭，手动检查与发布页下载兜底仍然保留。
- **按目标版本确认** — 如果提示出现后可用版本发生变化，Patchbay 会针对新的目标版本再次询问，不会沿用旧版本的确认直接安装。
- **导航与三层链路更清晰** — 侧边栏围绕技能库、安装、链路总览、项目链路、原件仓库、诊断和备份组织；重复的 Dashboard 与全局工作区入口已并入对应的主入口。
- **接入 Patchbay Central 与 Qoder Work** — 中央库安装的技能可以进入项目级三层链路，并支持 Qoder Work 的项目技能入口，同时不会把 Qoder 厂商管理的全局目录误报为策略违规。

### 开发者与治理更新
- 新增经 TDD 覆盖的更新协调器，注入设置、时间、更新器与进程边界；补充启动通知渲染测试，以及英文、简体中文和繁体中文完整文案。
- 注册 Tauri process 插件且只开放重启权限；发布契约现覆盖 updater/process 的依赖、注册、权限、公开端点、签名公钥和启动接线。
- 完成剩余的 CLI 决策工作流与 canonical instructions wrapper 合同，并在发版前删除退役生成资产和无用代码。
- 合并重复的导航与备份界面，移除未使用的 `@hello-pangea/dnd` 依赖，并按发布检查要求完成全仓 Rust 格式化。

## [1.29.3] - 2026-07-14

### 发布概览
- 从独立私有源码仓完整验证 Patchbay 的双仓发布架构。

### 用户可见更新
- **私有开发、公开更新保持稳定** — 签名后的 macOS 更新继续通过 `semantic-craft/patchbay-releases` 提供，不依赖公开源码访问。

### 开发者与治理更新
- 本版本完全由私有独立源码仓完成准备、打 tag、构建、签名、公证与发布派发。
- 发布专用 GitHub App 继续只为公开 release 仓签发短期、单仓库范围的令牌。

## [1.29.2] - 2026-07-14

### 发布概览
- 将 Patchbay 私有源码仓库与公开发布通道彻底分离。

### 用户可见更新
- **独立公开更新通道** — 应用内更新检查、版本链接和下载现在统一使用 `semantic-craft/patchbay-releases`；源码仓库保持私有的同时，签名后的 macOS 更新仍可公开获取。

### 开发者与治理更新
- 发布自动化改用独立的 Patchbay Release Publisher GitHub App 签发短期令牌；该 App 只安装在公开发布仓库，并且仅有 Contents 写权限。
- 私有源码工作流可以跨仓构建、签名、公证、验证并发布产物，不公开源码历史，也不使用可长期复用的个人令牌。

## [1.29.1] - 2026-07-13

### 发布概览
- 完成 Patchbay 独立产品身份迁移，并上线自有品牌、单仓库范围的 Patchbay Backup 连接。

### 用户可见更新
- **Patchbay 身份完整统一** — 桌面应用、内置与独立 CLI、数据目录、数据库、锁文件、备份元数据、文档和更新路由现在只使用 Patchbay。全新安装不再携带退役产品的数据迁移路径或浏览器状态键。
- **更安全的 GitHub 备份授权** — Patchbay Backup 采用两阶段 Device Flow：第一次授权只识别用户选定的私有仓库且不保存令牌；第二次由 GitHub 签发仅限该仓库的最终令牌。公开仓库、宽范围安装和扩大后的仓库访问都会被拒绝。
- **授权变化时明确恢复** — Git 操作前和令牌刷新后都会重新验证仓库私密性与令牌范围；授权撤销或范围扩大时会显示明确的重新连接操作，不再静默退化为空凭证。

### 开发者与治理更新
- 新增发布契约，扫描所有受版本控制的非二进制文件及路径，阻止退役产品身份重新进入交付面。
- GitHub App 重新授权错误现在会贯穿 system Git、libgit2、Chain pull 与 fork-sync；首次发现仓库所用的凭证不会进入设置、IPC、日志、URL 或系统钥匙串。
- 删除退役的默认数据目录、配置、数据库、元数据和 localStorage 迁移代码，同时保留当前用户主动修改仓库路径时所需的迁移能力。

## [1.29.0] - 2026-07-13

### 发布概览
- Patchbay 首个独立版本：以项目为中心的三层 Skills 控制面已在桌面端与 CLI 中完整落地，并建立带签名更新与 Apple 公证的 macOS 正式发布流水线。

### 用户可见更新
- **项目专属三层管理成为主工作流** — 链路总览、项目链路、原始仓库、Doctor 与全局守卫共用同一套术语和 Chain Service。用户可查看完整解析链、注册项目、预览并安全执行链接/解绑/迁移/规范化操作，仅对干净仓库做 fast-forward 更新，并通过重扫验证结果。
- **CLI 与桌面工作流对齐** — `patchbay-cli chain` 提供 topology、where、doctor、仓库健康、重复 checkout 比较、link、unlink、remediate、normalize、pull 和显式 fork-sync，并输出稳定 JSON 合同。
- **Patchbay 品牌与更新路由统一** — 应用内帮助以三层模型为主线，更新检查指向 `semantic-craft/patchbay`，官方 macOS 版本采用 Developer ID 签名并通过 Apple 公证。

### 开发者与治理更新
- 完成 issue #1 至 #20 的完整路线图：包括基于 adapter 的全局守卫、注册项目与多根目录清单、受保护的 plan/apply 写入、Doctor 决策持久化、仓库健康、CLI 对齐、GUI 收口、包装脚本收敛及能力到 ticket 的验证映射。
- 发布自动化先执行前端与 Rust 门禁，再构建两个 macOS 架构；使用 Patchbay 自有密钥生成签名更新产物，校验 Apple 公证、stapling 与 Gatekeeper，在 Release 仍为草稿时验证 `latest.json` 和签名，全部通过后才公开发布。
- 现有 helper 已成为 `patchbay-cli chain` 薄壳；各平台壳只保留政策选择，不再重复文件系统或 Git 变更逻辑。
