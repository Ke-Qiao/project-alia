# L11 Companion：事务参与边界清理

基线 `f15db66`，分支 `codex/architecture-refactor`。本包负责 Companion shadow/live repository 及其单元集成测试；Context source/invitation 实现交由 Context agent，Delivery 关联函数交由 Root。没有启动 Cargo、PostgreSQL、容器或外部服务，没有访问用户数据库或硬件，没有新增依赖、feature、迁移或策略开关。

## 本包 checklist：6/6

- [x] 审计真实调用者、事务所有者、锁顺序、commit ACK 与一次性 permit 边界。
- [x] Shadow/Live 消费者改用 Context `appraisal_source::{AppraisalReadAuthority, lock_appraisal_authority}` 和 Companion `source::load_companion_source`。
- [x] Live 使用 Context `invitation::issue_invitation_in_transaction`；直接调用 `DeliveryRepository::prepare_in_transaction(tx, input)`，删除只为参与同一事务创建的临时 repository / pool clone。
- [x] 扩充真实 PostgreSQL 原子失败回归：Context INSERT、Response INSERT、pending admitted UPDATE；验证前序写确已执行、具体故障标记、六张表基数和 pending 状态回滚；最后成功只有一次预算收费与一条 prepared audit。
- [x] 四个文件 scoped rustfmt、`git diff --check` 通过；两份生产文件的静态归一化比较通过。
- [x] Root 对最终同源快照运行编译、真实 PostgreSQL 和完整阶段验收，并记录最终结果。

## 保持的事务和政策

`LiveRepository::admit` 仍是唯一提交者，保持 READ COMMITTED 和两秒 statement timeout。顺序仍为 Context policy → active account → privacy → relationship → canonical source closure → Companion policy → pending row；预算、grant、invitation Context、Response、pending admitted 和最终期限检查仍在同一事务。只有 commit 确认后才能创建 Fresh permit；`Existing` 不带 permit。未改变原本的提交未知错误、退役、不退费、候选有效期、容量、默认 Shadow 或现行政策。

`CompanionRepository` 仍拥有 shadow evaluate/replay 事务，保持源验证与 shadow claim、operation/candidate、processed source、erasure root 的原子性。Shadow 不会创建 live 输出权限，已有 shadow 来源永不提升为 live。

直接调用者仍是 Companion service 和 LiveRunner；没有增加新的 trait、事务包装器、后台任务或池。测试中的每个确定回滚故障场景重新冻结机会，避免把多个 SQL 注入耗时挤进某个旧机会的两秒窗口；原生产重试和不可刷新期限逻辑未改。

## 验证入口与局限

修改的原子回归：`modules::companion::live::tests::atomic_admission_rollback_and_original_pending_lock_reconcile_without_second_permit`。它在注入每个故障后检查 `companion_admissions`、`context_leases`、`delivery_responses`、`delivery_events`、`companion_source_claims`、`companion_live_sources` 的基数与 pending 状态，并保留原 pending SQL barrier 的 rollback → 一个 Fresh → Existing 断言。最终 `budget_revision=1`，没有失败收费残留。

继续运行已有提交重认、预算、短期限、容量、shadow 防提升、源撤销以及 runner reserve 丢失/Existing 不执行 TTS 的回归。ACK 相关证据仍是控制真实事务提交/回滚的 barrier，不能称为物理网络 ACK 丢失模拟。

静态报告位于 `run/l11-companion-20261010/static-audit.json`：相对基线，两份 repository 的字符串字面量序列完全相同；剔除 use 导入并归一化批准的 API 名称/调用后，业务函数体一致。该检查只证明本包没有改动生产 SQL/判断/提交顺序，不替代 Root 运行验证。

本包未执行运行验证，最终集成状态以 Root checklist 和恢复检查点为准。真实模型、Pi/声学/机械动作和物理 commit ACK 丢失均未验证。

## Root 最终验收

源码 `60f665cf6fce4811b144dfc743b4814c5bdf19d5`，V4 manifest `520ec684c2619e775389330adbe837113edcb675e2f7d62fc7f5d65758fe274f`，640/640 Git blob 一致。macOS workspace **1060/0/5**、真实 PTY **3/0**、无默认 availability **2/0**、提前准入 **1/0**；Linux ARM64 Server 完整默认并行复跑 **778/0/5**，两端无默认全目标及 Linux release/native link 通过。所有最终结果同源且无漂移。

本轮 10/10，L09—L12 3/4，R01—R06 仍 2/6。四个任务容器均停止。停机前 schema/其他连接均为零，无需额外删除 schema。原失败、修正和最终证据见本目录 evidence-index.json。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械与其他平台。Linux V4 首次旧 panic-fence 子进程 20 秒超时仍保留为未定位项；定向和完整复跑通过不等于根因修复。
