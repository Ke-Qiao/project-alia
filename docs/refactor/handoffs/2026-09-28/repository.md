# Companion Repository / live admission handoff — 2026-09-28

分支 `codex/architecture-refactor`。本文件记录 CP05 的存储实施包与一个受管服务回归；不是整个 CP05、R01 或整工作区验收结论。Root 已报告冻结 V1 与正常提交 `a79cb1f`：630 个输入，manifest SHA-256 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`，记录于 `run/validation-20260928/final-source-manifest-v1.json`。本文件不自行宣布 CP07 完成。生产默认策略仍为 Shadow；本文件不授权在用户设备开启主动输出。

## 最新组件证据与进度

Root 的 [companion-v3.log](../../../../run/validation-20260928/companion-v3.log) 实际通过 **38 passed / 0 failed / 0 ignored**，默认测试线程，包含下列 **7 个真实 PostgreSQL live Repository 测试和 1 个 service 测试**，也包含其他 agent 的 Context、runner 以及既有 shadow/纯决策用例。日志记录编译 36.25 秒、测试 7.82 秒，Root 报告整个命令约 46.7 秒。该次早于冻结，不应单独称为冻结源或真实设备验收。

Root 后续报告同一冻结 V1 的 macOS workspace **1036 passed / 0 failed / 5 ignored**，另行实际通过 PTY 3 和 Easy-Turn 2，合计 **1041 个不同测试**；Linux ARM64 Server / Embedded no-default all-targets 编译均通过，Linux 本轮只有编译证据。上述 38 项属于整体测试的子集，不能再次累加。完整配对与声学边界由 Root/A/B 的对应报告说明。

本 agent 的组件清单为 **8/8 个已实施且在 V3 实际通过的回归**；总项目与 CP 主 checklist 的完成数由 Root 决定。

- [x] `atomic_admission_rollback_and_original_pending_lock_reconcile_without_second_permit`：真实 Response 写入失败回滚完整 admission + 派生 Context；生产事务体 staged 后，受控 rollback 允许同一不可变 intent 得到唯一 Fresh。第二次查询只得到 Existing。
- [x] `committed_original_transaction_returns_existing_and_cleanup_survives_revocation`：受控 production TX commit 后，等待原事务的重试只得到 Existing；账户禁用后，原 instance 的退休仍清理候选/grant 并标记 Response cancel_requested，保留 charge 与 source claim。
- [x] `last_daily_budget_is_serialized_and_erased_charge_is_not_refunded`：争抢最后一个 UTC 当日额度仅一项 Fresh；已退休 charge、新政策 revision 和新 source 都不能退款或绕过 cooldown。数据库拒绝删除 charge。
- [x] `shadow_sources_and_limited_worklist_never_upgrade_to_live`：五个 shadow 来源（包括一个 limited 来源）全部有不可变 origin claim；Enabled 不升级它们；新 instance 退休旧 pending，不重建机会或媒体。
- [x] `pending_capacity_original_expiry_and_source_erasure_remain_bounded`：owner 私有 pending 容量 4；排除 fenced ID 在 SQL LIMIT 前生效；候选保持原 source_received_at 与 expiry；完整 conversation 根闭包失效时全部依赖候选退休，第五条旧来源不借释放容量复活。
- [x] `commit_checks_original_short_admission_deadline_without_refreshing_output_deadline`：真实未提交准入事务持有至原 admit_before，COMMIT 的 deferred constraint 拒绝并回滚 grant、Context 和 Response；同 intent 后续退休，不刷新机会或候选期限。
- [x] `capacity_lock_serializes_distinct_owners_in_the_same_schema`：两个不同 owner 的生产事务仍受同一 schema 容量锁排序，使用 pg_blocking_pids 观察实际等待；原事务 rollback 后另一 owner 正常发现来源。
- [x] `same_channel_has_one_outstanding_snapshot_and_close_drains_its_original_attempt`：真实 PG + 受控 VoiceHandle mailbox，两候选共享 channel，保持首个 Snapshot reply 未返回，第二个不能竞争覆盖；关闭排空原 attempt，未创建 admission/Response/Delivery。

`stage_for_test` 仅在 `#[cfg(test)]` 可用，复用生产 `prepare_admission` 事务体并返回尚未提交的 DatabaseTransaction；它不创建 permit。事务屏障用例明确控制真实 commit/rollback，并观察另一个连接被原事务阻塞。原事务同时持有 authority/source/pending 锁，`pg_blocking_pids` 不区分重试首先等待的是其中哪把锁。**没有注入物理网络层的 commit ACK 丢失**，不能将这些测试标记成该故障已实测。服务用例也没有释放机会、调用模型、TTS 或播放；真实 actor 与双端组合由其他验证负责。

与 live-design 第 13 节的验收矩阵对应时，保留以下边界：

- 最后当日额度测试使用 `tokio::join!` 竞争两个真实事务，断言一 Fresh、一 DailyBudgetExhausted；没有专门强制该竞争的数据库等待屏障。UTC 换日和 awaiting_response 已有纯策略测试，没有本轮真实跨午夜或并发用户回应 PG 用例。
- 原子性直接注入的是 Response INSERT 失败，另有完整生产事务 staged rollback/commit 和 COMMIT 截止失败。未分别向 grant INSERT、Context persist 注入故障。
- live/Context 动态撤权覆盖 policy、message、privacy、relationship 与账户撤销后的 cleanup；完整 parent dependencies 有验证。9 个 shadow 测试的 Graph/Cache 等源族覆盖不能等同于所有 live trigger 都逐项做过动态故障和死锁竞争验证。
- runner 的 reserve ACK 未知测试丢弃受控 oneshot reply，该 fixture 不生成 OutputProducer；它证明原任务取消、无第二次预约/TTS、不退款，不证明物理设备 ACK 丢失或 SQL CommitUnknown 网络路径。
- 私有容量、过期/擦除与永久 charge/claim 是不同边界；永久 opaque 账本保留历史，没有被声明成全历史行数硬上限或长期容量 soak 已通过。

完整映射见 [存储合同第 7 节](../../companion-live-storage-design.md#7-actual-fault-evidence-and-remaining-boundaries)。本次只读核对未发现新的具体生产错误；未证实的更强断言继续开放，不能为提高完成数字而外推。

## 实施与接口

文件范围：

- `migrations/m20260926_000015_companion_live.sql` 与 migration 注册：独立 live pending / admissions / roots、shadow/live 共用 source origin claim、不可变身份与预算、原子擦除/取消、提交时截止检查。
- `modules/companion/live.rs`、`live/{types,repository,tests}.rs`：内部 live 类型、仓储、不可重建 Fresh permit 和真实 PG 回归。
- `modules/companion/repository.rs`：shadow 的全部 examined 来源（包括 limited）同 TX 写 shared source claims；discovery 不处理已被 live 声明的来源。已有 shadow fixture 只开放最少 test 可见性供本轮复用。
- `modules/companion/service/tests.rs`：单 channel snapshot 与关闭回归。`service.rs` 本体、runner、Builder/API 由 Root 持有。

主要内部接口：

```text
LiveRepository::new(db, VoiceInitiativeHandle::instance_id())
replace_policy(owner, expected_revision, policy)
discover_pending(limit)
discover_pending_excluding(limit, excluded_ids)
prepare_intent(candidate_id, original_voice_opportunity)
admit(&original_intent) -> Fresh | Existing | Deferred | Retired
receipt(owner, admission_id) -> opaque AdmissionReceipt
retire_candidate(candidate_id, reason)
retire_admission(owner, admission_id, reason)
retire_batch(limit)
```

所有扫描 limit 为 1..=32；排除集合最多 1024。`discover_pending_excluding` 在最终 SELECT 的 LIMIT 前排除受管任务已尝试的 ID，避免 SQL cleanup 暂时不可用时旧的第一批永久遮住后继来源。根权限、Context policy、privacy/relationship epoch 过滤复用既有 shadow 的 canonical 条件；扫描期间账户撤销按该项已不可用处理。

`LiveAdmissionPermit` 不支持 Clone/Deserialize，构造器位于 live 内部；只有 `admit` 的新写入得到成功 COMMIT ACK 后构造。getter 保持：admission_id、candidate_id、initiative_id、server_instance_id、context、opportunity、expected_scope、admit_before、deadline。查询 Existing 只返回 opaque receipt，绝不补发 permit。单次受管 attempt 保留同一个内存 `LiveAdmissionIntent`；不能在未知结果后更换 opportunity、request、Context、scope 或 output key。

测试复用入口为 `companion::live::tests::{Fixture, with_fixture}`，基于既有真实 source/Appraisal fixture。Fixture 提供 `live(instance)`、`enable_live(policy)`、`seed_pending(&repo)`、`opportunity(&pending)`、`fresh(&repo)`，以及 db/owner/session/context()/seed()/seed_for()/count()。`fresh` 不隐式启用政策，调用者必须先选择 Enabled 策略与 TTL。fixture opportunity 使用数据库时间和测试 VoiceScope，只是 Repository 输入，不是实际 actor 接纳证据。

## 事务、期限与清理不变量

所有准入写入使用 READ COMMITTED，依次取得原权限与完整 canonical roots、owner Companion policy，再对同一原 pending 行 `FOR UPDATE`，持锁至 grant + 专用 Context + Response 原子写入和 commit。重试取锁后发新的 SELECT 查询 admission；存在只返回 Existing。没有 admission、原 pending/instance/intent 仍有效，才允许同一个原 intent 再尝试。预算从全部已提交 admissions（包括 retired）读取，owner 权限锁将最后额度排序；charge 和 opaque identity 禁止删除。

私有容量为 pending 每 owner 4、全库当前 schema 128；grant 每 owner 128、当前 schema 1024。容量 advisory lock 使用 `hashtextextended(current_schema(), 固定域)`，与实际被计数的隔离表边界相同；同 schema 不同 owner 仍串行，独立测试 schema 不互相阻塞。没有为避免测试竞争改用单线程。

两段期限保持：

- `admit_before` 为原 candidate、机会、last_capture_at + 2 秒和原 UTC 日边界的最早时刻，保守截到 SQL 微秒；也不大于 Allowed 决策期限。准入事务写入结束后检查数据库 now，且 deferred SQL constraint 在 COMMIT 再严格检查 `clock_timestamp() < admit_before`。COMMIT 错误仍返回 CommitUnknown 原 identity，不假设是否成功。
- `valid_until` / permit.deadline 严格等于原 candidate.expires_at；不随扫描、重试或恢复刷新。首次 actor reserve 仍需独立严格检查 admit_before，客户端收到 COMMIT ACK 的时刻不与数据库 commit 原子。已建立的原媒体/Hold 使用固定输出期限与当前源/政策权限，不能因原短机会过去而重生成或无条件取消。

`retire_candidate` 和 `retire_admission` 只按原 instance/identity 取得 pending 锁，不要求当前账户/来源仍有新权限。统一 SQL 函数以 pending → admission → Response 顺序退休、擦除 private candidate/grant/digest/roots，并将规范 Response 标成 cancel_requested。它不向原 source 反向加锁，且不删除 budget/opaque identities。commit 未知但实际上已提交的关联 admission/Response 也包含在这条 cleanup 闭包中。当前隐私/关系/政策/根变更的 SQL trigger 使用同一函数；迟到事实与媒体 native 收尾仍由现有 Delivery owner 记录。

Root 已修复 review 指出的两点：同 channel 只保留一个受管 attempt，跳过 active channel 的候选也进入本轮 inspected 集合以便越过 SQL 首批；Existing 被原受管 attempt 观察后退休其无 producer 的计划 grant/Response，保留 charge 且不补发输出。

## 保留的失败与修正边界

- 初始编译发现 Delivery 类型导入路径错误，后续编译发现 TransactionTrait 被误从 live repository import 移除；均已纠正。没有把编译失败算作 PG 行为测试结果。
- V2 实际 **31 passed / 1 failed**。失败是本 agent 的 pending 容量测试把“修改最早消息后仍能保留/补齐 4 个候选”作为预期。后继 conversation Context 包含该消息的完整 canonical root，实际返回 0 正确。修正测试为断言完整闭包全部退休、旧第五来源不刷新、opaque claim 保留；没有放宽生产擦除。V3 通过覆盖修正后的断言。
- deferred-deadline 测试会真实持有交易直到短期限。初始 live 容量锁是整个数据库固定 key，会让本应隔离的测试 schema 互相竞争；已按实际 schema 边界分锁并保留同 schema 并发测试。此项是独立发现的锁范围问题，**不是 V2 失败的根因**。

## 接下来与资源

- [x] Root 报告冻结 V1、workspace / PTY / Easy-Turn 运行、Linux 编译通过并正常提交 `a79cb1f`；本文件只引用实际范围，不另行勾选 CP07。
- [ ] 真实 commit ACK 网络丢失、真实 Pi/声学双讲/AEC/尾音/长期稳定性仍不得视作已完成。
- [ ] 继续沿总 backlog 执行后续 R01/R02 等轮次；前端、通用 Embedded 适配、Pi 上 Easy-Turn 和跨机/同机完整部署实测依既有延期台账。

本 agent 未运行 Cargo/PG、未创建后台任务/容器，所有实际验证由 Root 统一窗口运行；本 agent 当前无执行会话需要清理。使用的是任务 PG 32768，Root 报告本轮重建后基线 schema 为空；没有访问用户 5433 数据库。本 agent 未自行 commit、push 或部署；Root 的本地提交见上文。

## Easy-Turn 恢复前只读核对（约 14:49 CST）

Root 后续要求核对模型服务：`127.0.0.1:8091/health` 连接失败，进程列表没有 Easy-Turn/Uvicorn 服务。现存 `services/easy-turn/.venv` 可运行 Python 3.10.20；安装版本为 torch/torchaudio 2.2.2、transformers 4.44.2、peft 0.13.2、uvicorn 0.34.2。

只调用 `alia_easy_turn.model.verify_artifacts`，没有构造 detector：官方 source HEAD `fe175eaa8442c9a0834acf29f8640ee74c2aceca` 且 clean；checkpoint 3,451,801,684 bytes，SHA-256 `1cb535bb0ffe3ffa0ac941bae4898aeb87adef7b88bb58f665af08eded6f6070`；Qwen 七项 artifact hash 均通过。模型 revision 为 `7d1f8b06776b575ac22e2fd4aa7913da2f4b8b98`，Qwen revision 为 `7ae557604adf67be50417f59c2c2f167def9a775`。无需下载或安装；这些只读检查不代表本轮模型加载/推理已经通过。

Root 后续已用原命令恢复，并报告 health ready、pins 一致；本 agent 没有启动。保留恢复命令供下一轮使用：

```sh
services/easy-turn/.venv/bin/alia-easy-turn \
  --source-dir run/models/easy-turn-source \
  --model-dir run/models/easy-turn \
  --qwen-dir run/models/qwen2.5-0.5b-instruct \
  --device cpu --port 8091 --inference-timeout 60
```

CLI 内部启用离线与 `local_files_only`；不需再次执行 prepare。ignored Rust 模型测试使用 `ALIA_EASY_TURN_URL=http://127.0.0.1:8091`、`ALIA_EASY_TURN_PCM=/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm/backchannel_real_1.pcm`、`ALIA_EASY_TURN_PCM_DIR=/Users/ke-qiao/Developer/project-alia/run/easy-turn-stream-pcm`。目录已有 complete/backchannel/wait 三份官方 PCM。不要将历史 cold/warm 延迟当作本轮实测值。
