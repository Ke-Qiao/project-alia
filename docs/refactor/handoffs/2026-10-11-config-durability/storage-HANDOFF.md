# Storage HANDOFF — 2026-10-11 配置发布持久性

分工：`skills_registry`；基线 `7165ad6`，共享分支 `codex/architecture-refactor`。上一轮 health 恢复检查点已读。本文件记录 storage 分工，不代表整轮验收通过。

## 状态与 checklist

- [x] 审计旧 OOBE writer、CLI fragments、配置 loader/merge 和全部实际发布调用者。
- [x] 与 Root / HTTP owner 对齐 `save` / `recover` / `has_pending` 和可 downcast 的 `PendingPublication`。
- [x] 实现原子、独占的 pending descriptor，文件、fragments 目录树及新建祖先同步。
- [x] 实现同意图 retry、marker 驱动重启恢复，以及外部变更拒绝覆盖。
- [x] 实现 marker 已删除但最后目录 sync 失败时的进程内恢复；无 marker 的普通 load 不新增 fsync。
- [x] 清理受控临时 descriptor，保持原 base symlink 的 canonical target/fragment 行为。
- [x] 编写 9 项 storage 单元回归和按目标隔离的 one-shot fault / stage barrier。
- [x] 仅对拥有的 Rust 文件执行 scoped rustfmt；与 HTTP / regression agent 完成接口对齐。
- [x] Root 统一编译与定向测试：V1 Server lib 已为 357/0/3；V2 完整父链修复现已纳入最终同源验收，见下文。
- [x] Root 双平台完整回归、无默认 feature、静态守卫和资源清理。
- [x] Root 更新整轮证据、checklist、恢复检查点并提交。

实现进度 **8/8**；上方最终验收与提交现为 **3/3**，最终结果见 Root 补记。

## 文件与调用契约

仅拥有 `packages/alia-server/src/config/publication.rs`、其 `publication/testing.rs` / `publication/tests.rs` 和本文。没有改 HTTP、OOBE flow、loader 接线或其他域文件。

`save(target, content, replace, fragments) -> anyhow::Result<PathBuf>` 承担一次完整发布。HTTP 使用 `replace=true, fragments=None`，CLI 首次安装使用 `replace=false, fragments=Some(...)`。输入 TOML/角色校验在 mkdir 之前；CLI fragments 目录必须独占创建，不复用或删除 operator 的既有目录。

`PendingPublication` 保留目标、私有请求指纹和私有 descriptor；`matches_request` 对齐原请求，`retry` 仅继续同次发布。API 可 downcast/clone，不暴露配置正文。已有有效 marker 但请求不同也返回原 typed pending，避免 setup owner 留在 Idle。错误明确表示 `durability is unknown (unconfirmed)`；任何 marker 发布后的失败都保留待恢复状态。

`recover(target)` 只对存在的固定 marker 启动恢复。`has_pending` 将存在或无法判断的 marker 保守视为需要处理。Root 在正常 loader / discover / needs_setup 处接入；恢复内部使用 `load_candidate`，不会递归调用 `load`。

## 发布与恢复顺序

1. 校验请求，创建本次拥有的候选 base / 可选 CLI fragments；base 为 0600，CLI fragments 继续按原 umask 创建。
2. sync 候选及全部实际 TOML fragments；目录由深到浅同步；同步 canonical target 的完整父链直到根目录。保存内容指纹与 Unix 身份/权限、文件集合、目录集合。
3. 将不含配置正文的 descriptor 写入 0600 独占临时文件，sync 后 hard-link 到目标旁的固定 marker，禁止覆盖其他 marker。sync 父链确认 marker 后才发布 base。
4. replace 使用同目录 rename；首次创建使用独占 hard-link。发布后确认 base/fragment 身份仍一致，再 sync base 和父链。
5. 数据已确认持久后，删除仅属于本 descriptor 的候选/临时记录、删除 marker，最后再 sync 父链。该末次 sync 失败仍返回 typed pending，保留内存 descriptor；重试不能把 marker 不存在直接当成功。

恢复允许两种磁盘状态：base 仍是记录的 original 且候选完整时继续原发布；base 已是记录的 prepared 时仅完成同步与清理。任何 target、candidate、fragment 文件增删改、fragment 目录集合变化或 marker 变化都会拒绝恢复并保留文件。候选必须是相邻受控 UUID basename 的 regular file，不接受候选 symlink；原有 base symlink 仍解析到原 canonical target，并使用该 target 的 fragments。

descriptor 临时名由 candidate UUID 唯一推导；marker 已发布后留下的该临时文件，只有 regular 类型且内容精确匹配当前 descriptor 才能被恢复清理。不会扫描或删除其他临时文件，也不会删除冲突的固定 marker。

## 回归清单与证据边界

9 项内部测试覆盖：

- 非法输入不创建父目录；marker 前同步失败保留旧 base/operator fragments，CLI 只清本次新树。
- 早期 FileSync 失败留下新父链后，第二次发布仍在 marker 之前同步完整父链，不把可见的残留目录误当作已持久。
- TOML 文件→最深目录→fragment 根→新建祖先→marker→base 的顺序与私有权限。
- marker 删除后错误、无 marker 普通 loader 纯读、持有原 descriptor 的重试再次 sync 失败与最终完成，且不重复发布。
- target/candidate/fragment 内容、增加、删除、目录增加的拒绝覆盖矩阵。
- 已有 marker 的不同请求保留原 typed pending。
- descriptor 路径逃逸拒绝与相邻原临时 descriptor 回收。
- 实际同步屏障下其他 publisher 抢占固定 marker，失败清理不能删除该 marker。
- Unix base symlink 与 canonical overlay 行为保持、candidate symlink 拒绝且不改外部文件。

`cfg(test)` hooks 按 canonical target 分区，支持一次失败和可释放阶段屏障，事件包含实际路径；Guard drop 清自己的映射并释放等待者。HTTP agent / regression agent 使用同一 hooks 验证实际 API、setup drain 和 loader 重建状态，未复制生产算法。并行不同测试不修改全局环境变量。

本分工未运行 Cargo、数据库、Docker、Git 或外部网络。Root 的 V1 冻结清单为 `44d404ec3ea37fc22ad657e882e33b5b212ad04de600f3a535c23e4945c488ff`（650 输入），Server lib 已完成 exit 0，**357/0/3**，无源码漂移。V2 新增完整父链修复及第 9 条回归后的结果仍待 Root 追加；进程内故障注入不能替代真实断电实验。

## V1 → V2 的主动复核修复

V1 定向测试通过期间发现原 `create_parent` 计数存在重试缺口：首次请求创建新父目录后在 candidate 的 FileSync 处失败，会留下可见但未确认同步的空目录；第二次请求把这些目录视为既有祖先，使同步范围缩到当前父目录，可能漏掉上次创建的祖先 entry。既有 8 项测试未覆盖这条跨请求失败路径，不能凭 V1 通过忽略该缺口。

Root 确认 V1 Cargo 全部退出后，V2 删除 `ancestor_count` 和 `create_parent` 计数，每次发布/恢复直接同步完整 canonical 父链。新增第 9 项测试先制造早期失败，再逐一断言第二次操作在 MarkerDurable 之前对每个祖先执行 DirectorySync。Root 的当前 macOS 探针确认完整链直到 `/` 的 std 同步成功；其他实际平台结果待统一验证。

## 保留的明确边界

- 这是配置域一次发布恢复记录，没有引入泛用 journal、跨域事务、依赖、feature、平台或硬件动作。
- 在 marker 发布前崩溃，候选/临时 descriptor/CLI 树可能成为缺少持久所有权证据的孤儿；自动恢复不会冒认、扫描或删除，需人工核对。失败时的 best-effort 准备清理也不承诺删除操作本身的断电持久性。
- 信任部署账户控制的本地文件系统；没有增加防恶意同账户并发换 symlink/内容的系统级排他机制。正常无 marker 读取保持只读；保存/恢复要求完整 canonical 祖先目录可打开并同步，权限或文件系统不支持时明确失败，不提权或忽略同步错误。
- 同步顺序不等于所有文件系统/介质在真实断电后的保证；其他平台和硬件验证继续延期。
- 既有 SettingsRegistry 普通设置保存不在本轮配置首次发布/恢复范围。

## Root 最终验收补记

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

上文 subagent 实施阶段的进度数字与未运行说明保留历史时点；对应 Root 验收事项已勾选，现已完成同源整合验收、资源关闭与源码提交，本轮总 checklist 为 10/10。

未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。
