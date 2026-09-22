# DU08 · 持久控制账本验收

范围：独立分支 `codex/duplex-control-ledger`，从 WIP `76f5b04` 恢复。
本 checklist 只验收 Repository/SQL 权限与事实，不替代 DU03–DU10 的运行时、协议和
两端播放整合。证据目录为本工作树 `run/validation-output-control-20260922/`。

## 身份、权限与期限

- [x] 读主恢复检查点和自身 HANDOFF，核对原分支/SHA；6 项 Core-only 临时覆盖保留且排除提交。
- [x] 只读核对 `alia-refactor-test-db` 正在运行、绑定 `127.0.0.1:32768`，库为
  `alia_refactor_delivery_final`，用户为 `alia_test`；初始 3 个历史 pipeline schema
  已记录于 `db-baseline.txt`。不访问用户 5433。
- [x] 规范 Response 的 source/Context/voice scope 与原 Server instance/backend 必须一致；
  可以在 first Start 前登记；重复登记不替换身份或刷新期限。
- [x] 同 scope 被另一规范 Response 占用时返回 typed `Conflict`，唯一行仍指向原 Response。
- [x] 同 command ID 同意图只查原记录，异意图拒绝；首次明确提交成功才返回一次许可，
  真正 PostgreSQL COMMIT ACK 丢失后返回 Unknown，重试没有新许可。
- [x] Pause/ConfirmExplicitWait/Resume 分开，序号连续且最多 32；Confirm/Resume 不延长原 hold。
- [x] 注册之后加入的 AudioDelivery 观察截止时间限制新控制许可；到期后拒绝新 Resume，
  既有规范媒体事实不变，登记重试不刷新原 lifetime。
- [x] 活跃 pending/sending/observing 的原 audio owner 租约到期时拒绝新控制；
  已提交终态不受过往 writer 租约阻断，原规范晚事实仍能记录。单项真实 red→green。
- [x] 完整原 Context 根在新控制事务中验证；privacy/policy、真实 Appraisal/relationship 根、
  Memory revision/private/expiry、cancel、inactive owner 的撤销都阻止新 Resume。
- [x] 生产 `AliaMemory.clear`、`clear_relationship`、policy CAS、`request_cancel` 与
  Resume 的真实双事务屏障通过，使用 `pg_blocking_pids` 确认等待而非靠 sleep 猜测。
  已进入 gate 的事务先提交；之后新命令被撤销。

## 回执、未知结果与资源边界

- [x] 原 immutable outcome 与 current snapshot 分离；迟到首次结果可保存，不回退更高观察。
- [x] snapshot 的 command sequence 不能高于数据库已经接纳的 command watermark。
- [x] 每 command first/latest 加 scope latest，总共最多 65 份回执；270 次连续观察更新（序号推进至 290）不增加
  只读轮询审计，旧观察返回 Stale，不把淘汰误称已验证 duplicate。
- [x] 审计 INSERT 故障时命令接纳/水位或首次 outcome/当前 snapshot 在同一事务回滚。
- [x] 撤权或本地永久退休后仍接受严格原身份的晚事实；晚 Running 不能复活本地权限。
- [x] 只恢复真实到期记录，不接管另一活跃实例、不重新 reserve/dispatch、不写第二条
  Response/assistant history，也不据控制回执制造 Presented 事实。

## 命令与证据

所有命令在账本工作树执行，`CARGO_TARGET_DIR` 指向主仓 target；真实 PG 命令设置
`ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@127.0.0.1:32768/alia_refactor_delivery_final`。

- [x] `cargo test -p alia-server --test output_control --locked -- --nocapture`：
  初始 `pg-baseline.log`，7 pass / 2 fail，exit 101。两失败为已修正的夹具根与同步边界。
- [x] 同命令：`pg-known-gaps-red.log`，9 pass / 3 fail，exit 101；三处已知生产缺口真实复现。
- [x] 同命令：`pg-fixed.log`，12 pass / 0 fail / 0 ignored，exit 0；测试体 4.36 秒，
  含编译命令总耗时记录于 `pg-fixed-result.json`。计数不与失败候选累加。
- [x] `cargo test -p alia-server --test output_control --locked expired_active_audio_owner -- --nocapture`：
  `pg-owner-lease-red.log`，0 pass / 1 fail，exit 101；原active owner失效问题真实复现。
- [x] 完整目标同命令（不带单项filter）：`pg-final.log`，**13 pass / 0 fail / 0 ignored**，
  exit 0，测试体 3.94 秒，总耗时 15.07 秒。此前候选结果保留，不累计为最终统计。
- [x] 默认 `cargo check -p alia-server --all-targets --locked`：
  `check-final-default.log`，exit 0，24.76 秒。
- [x] 无默认特性 `cargo check -p alia-server --all-targets --locked --no-default-features`：
  `check-final-no-default.log`，exit 0，24.18 秒。
- [x] 最终 `source-manifest.json` 记录 7 个 owned 文件与 6 个原 overlay 的 SHA256；
  overlay 哈希与上次 HANDOFF 全部一致。`rustfmt --check`、`git diff --check` exit 0。
  本分支仅提交账本/测试/相关文档，提交映射见同证据目录 `commit-result.json`。
- [x] `db-final.txt` 与 `db-baseline.txt` 的 `cmp` exit 0；schema 与初始基线完全一致，
  仅保留原 3 个历史 pipeline schema，目标库无其他活动 backend。fixture 全部清理，
  未创建容器、未人工删除 schema；PG 窗口已经释放。

## 由纵向整合包继续验收

- [ ] 最新 Core/Proto、runtime owner 与账本组合后的默认/无默认编译及真实 gRPC 测试。
- [ ] runtime 遵守每次 `ControlPermit.deadline_at()` 并在永久退休后实际停止旧媒体；
  持续心跳、暂停时 producer/native 生命周期、3 秒临时语义和真实模型两类暂停由主代理验收。
