# CompanionInvitation 历史只读引用 — 独立审查

2026-10-17；基线 `95cd077`，既有源码 `9e3a934`，本轮源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。审查进度 **5/5，最终 GO**：静态审查、定向与完整回归原件、资源回收、提交及归档身份均已独立核对，无阻断。审查者未运行 Cargo、数据库、设备或硬件操作，未修改生产或测试源码。

## Checklist

- [x] 核对用户确认的持久历史语义、既有 invitation purpose 和输出授权边界。
- [x] 审查七个生产文件的规范身份、根闭包、期限、锁顺序及 Memory 持久化。
- [x] 追踪实际模型历史调用链和派生消费、防 Companion 自我强化边界。
- [x] 独立复审七项测试；修正已确认的前置错误及反例见证后给静态 GO。
- [x] 核对 Root 统一运行的 RED／GREEN／全量原件、冻结输入、资源回收及最终提交身份。

## 静态结论

当前候选符合本轮范围，未发现阻断。`CompanionHistory` 是已提交邀请文本的只读证据；它不复用 `CompanionAdmission`，不恢复 private grant、播放、执行批准或主动输出资格。原 `CompanionInvitation` purpose 仍仅授权原来的两种输出用途。新的 Generation 仍必须具有自己的真实用户来源、请求身份和当前政策授权。

`materialize` 的直接历史入口限定同一所有者、会话和频道；新 marker 保留原 assistant Message、Response、Context 和文本摘要。消费时重新核对规范正文、Response 的 committed 状态和不可变邀请身份，采用数据库微秒时间精度比较。输出期限到期、原 grant 退休和切回 shadow 不单独使历史失效；原非 admission 根的 TTL、内容、privacy epoch 和当前 Context 政策仍然生效。派生 Context 的期限被这些真实根期限收紧，不沿用已结束的输出 TTL。

`history_roots` 保留完整非 admission 根闭包，拒绝嵌套 invitation marker／admission；通用证据验证继续限制 admission 的用途和数量。`bind_memory_roots` 持久保存 marker、完整原根和最早期限，使后续 synthetic Memory 验证仍能回溯原邀请。synthetic 验证不要求当前人工构造的会话身份等于原历史会话，但仍验证原历史自身的完整身份和当前所有者、政策与 epoch。不能通过 Memory／cache 丢掉 marker 后重新成为 Companion source；`load_companion_source` 明确拒绝两种 invitation 证据。

实际生成链路为 `interaction/context.rs::ensure_turn_context` → Context materialize → `generation.rs::prepare_turn` 的 `history_messages()` → `RouterInput.conversation_context`。现有普通、技能与升级路由使用这一共同历史来源，后续提交与输出仍使用当前 Context 验证。没有发现需要另建注入入口的遗漏。

## 锁与撤回边界

最终 `read_history_identity` 对 admission 使用普通 SELECT；相关身份字段由现有 SQL 不可变约束保护，读取不依赖 admission 的退休状态或 grant。assistant Message 与 Response 分别加共享锁，不再为了只读身份额外锁 admission，避免引入不必要的退休锁耦合。曾检查原设计的相反锁序风险；未把邀请自身必然造成死锁当成已复现缺陷。

本轮保证消费时拒绝失效根。原始 Message／Response 的裸 SQL 删除或修改，不等于物理删除所有已派生 Memory、cache、Appraisal 或审计记录；现有管理读取接口也不因此获得新的擦除保证。明确 privacy／relationship 流程继续沿用既有物理擦除。现有 SQL 触发器没有一个可通过简单增补 `kind='derived_message'` 分支就覆盖全部后代擦除的统一机制。本轮没有扩大到全局擦除设计，契约文档已写明此限制。

## 测试审查与已修前置问题

七项测试覆盖：真实新 Generation 的直接历史、未提交／跨身份排除、输出 TTL 与根 TTL 分离及 grant 退休／shadow、根和规范正文／政策／隐私撤回、派生 marker 不能省略或伪造且不能夹带 admission、真实 Memory 绑定存储与 cache 消费撤回，以及后续 Appraisal 不能产生新 Companion source。

独立审查提出并确认已修复：

1. 原测试使用新的随机 server instance 调用 `retire_candidate`，不能退休原实例 admission；改用 `permit.server_instance_id()`，并断言退休状态和 grant 清空。
2. 原 source 裸删除会被既有 Appraisal 外键拒绝；删除场景明确先删除相关 Appraisal，再删源，标记为复合撤销，并另加 source 内容更新验证独立来源变更。
3. anti-loop 由宽泛 `is_err()` 改为断言特定拒绝原因 `an invitation cannot become a new Companion source`，避免其他错误造成假阳性。

测试证据的解释限制：Context 重建加显式原实例退休验证持久读取，不是实际进程重启测试；cache 场景直接种入规范 active 行，不证明缓存审核全流程；Memory 场景验证实际绑定／存储、持久根和 synthetic 消费拒绝，不证明裸删除后的物理擦除或管理列表隐藏；Execution 复用公共 Context 验证，本轮没有新增真实执行链测试；committed 文本不证明用户听到了音频。

## 已核对定向证据

已独立读取 `run/validation-20261016-invitation-history/` 下 RED／GREEN 原始日志、运行记录和各自资源快照：

- RED：`0 passed / 1 failed / 0 ignored / 377 filtered`，测试体 0.47 秒、命令 66.77 秒，exit 101。唯一失败为已提交同会话 invitation 被普通 Generation history 遗漏的目标断言；不作为回归通过数累计。
- GREEN：`13 passed / 0 failed / 0 ignored / 371 filtered`，测试体 5.63 秒、命令 49.54 秒，exit 0；其中七项为本轮新测试，六项为既有 invitation 测试。anti-loop 源码断言特定拒绝原因，该测试日志为通过。运行记录保持默认测试并发、`--offline --locked`，`source_drift=[]`，进程 watchdog 未到期。
- 两次运行后同一任务 PG 容器快照均为 `schemas=0 / other_clients=0`。这些是运行结束后的观测，不是声称结束当刻原子快照。

独立重算 GREEN manifest SHA-256 `b6b481d699aa752e7a98aad2a50ea5e9f966b902d73e2ee48a4a0f0751ff6e5d`，全量候选 manifest SHA-256 `4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`；两者各 677 个输入，逐项内容完全相同，当前输入重算无漂移。两份 manifest 的元数据时间不同，不代表源码差异。保留的 GREEN 二进制 102,991,360 字节，其 SHA-256 与 `green-binary.json` 的 `723f38348e61bda8bd32c708a34c92d50a7528a026aa0efb09f90f97691dda1a` 一致。

## 完整回归、回收与身份闭环

独立按 Cargo 的 67 个 `Running` 和五个 `Doc-tests` 目标分段，取每目标最终结果，重算为 **1277 passed / 0 failed / 5 ignored / 0 measured / 0 filtered**，与 `validation-summary.json` 一致。日志内三条子进程重入结果属于现有测试内部执行，不重复累计。RED 和定向 GREEN 也不加进最终通过总数。`server_runtime` 为 85/0/0，`duplex_playback` 为 31/0/0；本轮完整命令 555.32 秒、exit 0、默认并发、`--offline --locked`、无源码漂移，900 秒进程 watchdog 未触发。五项 ignored 仍为既有三个真实 PTY 用例和两个真实 Easy-Turn 服务用例，本轮没有启用这些外部条件。

最终保留的 Server library 测试二进制为 102,991,360 字节，独立 SHA-256 为 `9f1c2c3585c4124a9acf1198ffbb9e8e46ef3cd2685045ad0d9e4473c84dbe25`，与 summary 一致。定向 GREEN 与 workspace 使用各自记录的二进制；未把二者误称同一个文件。

`final-resources.json` 于 `2026-10-16T16:09:44.490426Z` 观测原任务 PG 为零 schema、零其他连接。按运行起点与秒数计算，命令结束约为 `16:09:44.231986Z`，快照在其后约 **0.258 秒**，不是结束当刻快照。`cleanup.json` 原件显示先核对完整容器 ID `0473e0746cba7b0964c110b6de8013f202597d3b5a6bac1c5c50910e717bd867` 和 label `invitation-history-20261016`，再成功执行 stop、`rm -v` 并查询确认不存在；末次容器列表仍保留共享 astrbot。审查未再次操作任何容器。

独立读取源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc` 的全部 677 个对应 Git blob，逐项字节数及 SHA-256 与最终 manifest 一致，范围内无 extra／missing 输入。独立重算 [evidence-index.json](evidence-index.json) 所列 **34 份归档**，大小和 SHA-256 全部匹配，且均与 `run/validation-20261016-invitation-history/` 原件逐字节一致；index 指向同一源码提交。

最终 GO 仅收口本轮 invitation 只读历史交付。它不宣称历史 V01 已关闭、Linux／Pi 已在本轮重验、实际进程重启已验收、真实模型或硬件已联调。默认并发、原期限、default shadow、政策／权限及 R03 暂停保持；没有新增依赖、平台或真实硬件动作。
