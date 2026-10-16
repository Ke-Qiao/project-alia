# CompanionInvitation 历史只读引用 — Ownership

2026-10-16 开始，2026-10-17 收口；基线 `95cd077`，生产源码基线 `9e3a934`，最终源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。负责契约追踪及获授权的七个生产文件；测试由 flow 编写，Root 统一执行与提交。本交接 **6/6 完成**，不是项目整体完成率。

## Checklist

- [x] 追踪历史 materialize、证据校验、Companion grant/source、Memory/cache/Appraisal 消费边界。
- [x] 确认用户选择：已提交历史独立于输出 TTL、服务重启和改回 shadow；来源撤回、隐私和当前对话政策仍有效。
- [x] 只读核实 Root 保存的旧实现 RED：唯一新用例失败于遗漏邀请历史，失败后同库 schema/client 均为 0。
- [x] 完成七个生产文件并核对 evidence enum 消费者，交给独立 review；保留原权限、期限、并发及硬件边界。
- [x] Root 统一定向 GREEN 通过；独立复核原始日志、资源快照、保留二进制与最终冻结输入一致性。
- [x] Root 完成最终源码冻结、全量回归、资源回收与正常提交；独立复核最终日志、二进制、归档及 677 个 Git blob 一致性。

## 实现与归属

1. `packages/alia-server/src/modules/memory/context/types.rs`：新增 `CompanionHistory { id, response_id, context_id, content_digest }`。它是只读证据，不是新的 Context purpose 或输出 permit。
2. `packages/alia-server/src/modules/memory/context/materialize.rs`：普通 Generation 历史沿用原路径；直接邀请历史额外要求同 owner/session/channel，合入原邀请全部非 `CompanionAdmission` 来源及新的只读引用。保留现有时间窗口、条数/字节/证据上限和失效候选整体跳过规则。
3. `packages/alia-server/src/modules/memory/context/repository.rs`：为只读引用增加独立校验及排序键。原 `CompanionAdmission` 必须恰有一个且仅配 invitation purpose 的规则、通用 parent validator 均未放宽。
4. `packages/alia-server/src/modules/memory/context/invitation.rs`：Context 拥有历史正文、规范 Response、原 lease 及完整来源闭包的校验。历史要求已 committed 的固定邀请内容与 assistant message、原内容摘要和身份一致；不将 commit 解释为设备已播放。
5. `packages/alia-server/src/modules/companion/invitation.rs`：只读取 admission 的不可变身份列，复用现有 SQL 不可变/禁止删除约束；不读取或保留已退休的私有 grant，不要求当前 live 模式或 active 状态。该读不取得 admission 锁，不让历史读取等待其退休写锁。
6. `packages/alia-server/src/modules/companion/source.rs`：拒绝携带 `CompanionHistory` 的来源，防止历史邀请重新触发主动表达。此规则保守地同时阻止携带该历史闭包的后续 Appraisal 再成为 Companion 来源。
7. `packages/alia-server/src/modules/memory/context/memory_appraisal.rs`：Memory 绑定明确保存只读引用及原邀请全部非 admission 根、真实最早期限；现有 self Memory ancestor guard 保持。

`history_roots(tx, parent)` 排除邀请自己的输出截止，重新取得 Memory、Graph、Cache、普通 Derived/Execution parent、Appraisal/Affect 及其 parent 的实际最早期限，并保留失效检查。候选 Generation 与派生 Memory 继承该期限，不能以“历史可读”为由延长来源有效期。

持久 evidence 可能经 Memory/graph 的 synthetic Generation（没有当前会话）校验，因此只读 validator 校验历史自身的 Message/Response/Context/admission 会话和 owner 一致，不把 synthetic 当前会话与历史会话作比较；直接 materialize 仍必须同会话。来源时间与 admission 截止身份比较沿用 PostgreSQL 微秒精度，不改实际期限。

锁顺序保留政策/account/privacy/relationship 与原来源闭包；admission 仅普通读取不可变列，assistant message 和 Response 正文分别加共享锁。此前仅识别出不必要的新增 admission 等待边，没有动态复现或宣称修复既有死锁。

## 调用与权限复核

- `generation.history_messages()` 已是所有模型历史调用的共同入口；无需修改 Interaction 生成流程或另建历史入口。
- `ContextEvidence` 的穷尽匹配位于 repository、Companion source 及本次历史期限计算，均已覆盖新 variant；其它 Memory/cache/voice 的定向匹配保持原语义。
- `memory_write_context` 仍只索引真实 UserMessage 和 Memory ID；只读历史闭包由 `bind_memory_roots` 单独保留，不能把邀请 assistant message 伪装成用户输入。
- 原邀请 `authorizes` 仍仅允许 ResponseCommit/OutputAdmission；Memory、Execution、Cache/Appraisal 的新请求依赖自己的正常 Generation、当前政策和批准。voice initiative 仍要求专用 invitation Context 与原 admission 身份。
- 普通 Generation 的后继 `DerivedMessage`、Cache、Memory 和 Appraisal 会继续携带只读 marker；引用失效时不能继续授权派生消费。

## 已读验证证据

`run/validation-20261016-invitation-history/invitation-red.log`：

- `committed_live_invitation_is_read_only_history_for_a_new_generation_request`：0 passed / 1 failed / 0 ignored / 377 filtered，目标 0.47 秒。
- 唯一预期断言：`committed same-session invitation was omitted from ordinary Generation history`。
- `invitation-red.json` 的 `source_drift=[]`；`after-red-resources.json` 为 0 schema / 0 other clients。

`run/validation-20261016-invitation-history/invitation-green.log` / `invitation-green.json`：

- Root 单次执行 invitation 模块定向回归：**13 passed / 0 failed / 0 ignored / 371 filtered**；目标 5.63 秒，命令 49.54 秒，默认测试并发、退出码 0、`source_drift=[]`。
- 原有 6 项邀请权限/输出回归与新增 7 项历史回归全部通过；新增覆盖同会话只读历史、退役/过期/重建/shadow、owner/session/channel、正文/来源/政策/隐私、marker 闭包与伪造 admission、Memory/cache 消费撤回及 Appraisal 不可再作 Companion 来源。
- `after-green-resources.json` 为同一任务容器 `0473e0746cba7b0964c110b6de8013f202597d3b5a6bac1c5c50910e717bd867` 的 0 schema / 0 other clients。
- 独立重算保留的 `green-server-library`：SHA-256 `723f38348e61bda8bd32c708a34c92d50a7528a026aa0efb09f90f97691dda1a`，与 `green-binary.json` 一致。
- 独立比较 GREEN 与最终 workspace 的 source manifest：**677 项输入的文件列表和摘要完全一致**。GREEN manifest 文件 SHA-256 由执行记录固定为 `b6b481d699aa752e7a98aad2a50ea5e9f966b902d73e2ee48a4a0f0751ff6e5d`。

最终证据已归档至本交接目录的 [evidence](evidence/) 与 [evidence-index.json](evidence-index.json)：

- Root 单次完整 `cargo test --workspace --offline --locked`：**1277 passed / 0 failed / 5 既有 ignored / 0 filtered**，555.32 秒，默认测试并发、退出码 0、`source_drift=[]`。本 agent 按 Cargo target 的最后一条结果独立重算原始日志，与汇总一致；未叠加 RED、定向 GREEN 或子进程重入计数。
- Server library **381/0/3 ignored**，7 项新增历史回归全过；`server_runtime` **85/0/0**（80.44 秒）；`duplex_playback` **31/0/0**（45.95 秒）。5 项既有忽略分别为 3 项真实 PTY 与 2 项真实 Easy-Turn，当前轮未执行。
- `final-resources.json`：同一任务 PostgreSQL 在命令结束约 0.258 秒后为 **0 schema / 0 other clients**。`cleanup.json` 确认核对 ID/label 后只删除本任务容器及其卷，任务容器已不存在，其他容器和共享运行环境保持。
- 最终 manifest SHA-256：`4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`。独立通过 `git cat-file --batch` 读取提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc` 的 **677 个源输入 blob**，字节数和 SHA-256 全部匹配冻结清单；记录同时确认无额外或缺失输入。
- 独立重算最终保留二进制 `run/validation-20261016-invitation-history/final-server-library`：SHA-256 `9f1c2c3585c4124a9acf1198ffbb9e8e46ef3cd2685045ad0d9e4473c84dbe25`，与最终汇总一致。
- 独立复核 `evidence-index.json` 的 **34 份归档原件**：字节数与 SHA-256 全匹配。可执行二进制仍保留在忽略目录 `run/`，归档保留其身份记录，未将二进制声称为已入库文件。

本 agent 未运行 Cargo、测试、数据库命令、格式化、硬件操作或 Git 提交；本轮收口仅修改自己的交接文档。完整软件回归通过不替代 Linux/Pi、真实模型、硬件与长稳验收。

## 明确边界与回归重点

- 保留默认 shadow、当前政策、工具可见性、执行批准、精确取消和资源清理；R03 继续暂停。无新依赖、新表、迁移、平台或硬件动作。
- 复用真实 privacy erase 与 relationship clear。现有裸删 Message/Response 不具备通用的所有派生数据物理擦除框架；Memory 管理 `get/list` 也不逐项验证根。本轮保证来源失效后不可新引用，既有 Generation/cache/Memory/Appraisal 不可继续授权，不承诺裸删 SQL 后物理擦除所有后代，不扩张全局管理 API。此既有边界不是用户要求延期的已修复项。
- 应回归：退休且私有 grant 已擦除后的历史读取与原输出拒绝；来源/Appraisal/Memory TTL；跨 owner/session、内容篡改、遗漏 canonical roots；二级 Generation 的父闭包保留 marker；派生 Memory synthetic 校验、cache 复用及 Appraisal 来源拒绝；隐私/关系清除。
- 新只读 marker 自身不证明任意被篡改 SQL 的 materialized 历史；信任边界仍是数据库不可变 Context、规范 Response 提交和受控服务写入。不要声称可以抵御任意直接改写数据库且重算摘要。
- 历史 V01 25 秒超时根因仍独立开放，本轮不能倒推该根因。
