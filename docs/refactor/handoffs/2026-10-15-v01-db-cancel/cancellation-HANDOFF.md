# V01 数据库初始化取消：确定性验证交接

日期：2026-10-15。基线：`aca6ca0`。负责分工：`v01_flow`。当前阶段：本分工全部完成。两项旧实现 RED、首次定向 GREEN、首轮全量失败及修订后完整 V2 验证均独立保留；最终源码提交为 `8bcda7af892ba36aea98b9de785afa68e3935940`。

## 本轮范围与已确认缺口

本轮只处理 Delivery 测试 fixture 初始数据库构造的取消边界。原 `Fixture::new_with` 的 `catch_unwind` 可以捕获初始化 panic，但整个构造 future 被 drop 时不会执行后续 `Fixture::cleanup`。此前三项初始化失败测试验证的是 panic／SQL 失败，不能替代取消测试。

原 `InitializationStep::WorkPool` 位于 `CREATE SCHEMA` 确认后、工作 pool 创建前；`UserSeed` 位于工作 pool 与迁移完成后、插入用户前。因此可直接利用既有每次调用的 checkpoint 准确取消，不需要猜测 sleep、改期限、改并发或注入生产故障。

这些缺口属于测试 fixture 的所有权与清理；没有证据将其归因为历史 Companion V3 25 秒超时。默认 shadow、当前政策、原 25／45 秒期限和 `CASES=3` 均保持，R03 继续暂停。

## 已实现的旧实现 RED

独占源码文件：`packages/alia-server/tests/duplex_playback/initialization_cancel.rs`。

- `cancelled_after_schema_creation_removes_schema`：在原 `WorkPool` checkpoint 取消整个 constructor。
- `cancelled_after_work_pool_creation_closes_pool_and_removes_schema`：在原 `UserSeed` checkpoint 取消整个 constructor。

两项均先创建独立 observer；checkpoint 保存该次随机 schema 与工作 pool clone，以 oneshot 通知已到达，然后保持 pending。主测试同时 poll 原 `probe_initialization` 和通知，收到后直接 drop 整个 pinned constructor。局部 Drop sentinel 记录 checkpoint future 已销毁，避免把注入 panic 冒充取消。

取消后先查询原随机 schema 是否存在、工作 pool 是否仍能执行 `SELECT 1`，并打印阶段与固定布尔证据；再关闭工作 pool、通过 observer 删除原 schema、关闭 observer，最后断言。这样旧实现留下的资源会被观察并记录，RED 失败不会污染后续测试。

Root 已运行并归档 RED，本分工只读独立核对：`run/validation-20261015-v01-db-cancel/cancellation-red.{json,log}` 显示 0 通过／2 失败，测试 0.26 秒、命令 12.04 秒，默认测试并发且 `source_drift=[]`。两项均 `checkpoint_dropped=true`、`schema_remains=Some(true)`；`UserSeed` 另有 `work_pool_remains=true`。失败均为取消后 schema 残留断言。`after-red-resources.json` 显示 fallback 后同一 PostgreSQL 为 0 schema／0 其他连接。

旧 helper 与测试分别归档在 `red-delivery-support.rs.txt`、`red-initialization-cancel.rs.txt`，源清单为 `red-source-manifest.json`。本分工未运行 Cargo、PostgreSQL、SSH 或硬件，也未提交。

## GREEN 验证约定

另一分工已实现独立 `FixtureInitialization` owner，保留正在执行的真实数据库操作直到结束，并在取消时清理后提供可等待的 completion。两项测试已适配新 API：`WorkPool` 将 owner 移进构造 future，再 drop 整个 future；`UserSeed` 创建借用 owner 的 `ready` waiter但完全不 poll，等独立 worker 到达原 checkpoint 后 drop waiter并保留 owner。后者覆盖“未 poll 的 waiter 取消也必须生效”。两项都保留原 checkpoint、Drop sentinel、原 schema／pool 观察和最终断言。

另新增 `cancelled_buffered_ready_cannot_be_retried_as_a_closed_fixture`：使用标准库 `Wake` 与 Tokio `Notify` 为 ready 手动注册一次 waker，记录第一次 poll 为 pending；等待真实 UserSeed checkpoint 进入并放行后，才在原 5 秒观察窗口内等待 worker 的 send 唤醒，保持 ready 消息已缓冲但未被消费，记录原 owner 仍 pending，然后 drop waiter。重试 ready 必须返回固定的初始化取消 panic，不能交付随后已关闭的 Fixture。测试捕获该 panic、join 同一 completion、保存并清理原资源，最后断言。无需新依赖、额外 Ready checkpoint 或修改生产 API。

取消后必须显式等待原 owner 完成，之后才查询资源。不能用 drop 后立即查询或猜测 sleep 替代 join，不能将仍在清理的资源误报为泄漏。工作 pool clone 仅为观察证据；显式 close 应关闭共享 pool，使该 clone 也无法继续使用。并发许可需保留到 owner 清理完成，不能因外层等待方 drop 而提前放行。

初始化原本位于 body 的 45 秒预算之前。本轮不添加或延长成功期限；现有 body＋context cleanup 的同一绝对 45 秒预算及超时后保留清理 future 的规则继续保持。

## CREATE 在途验证（实际定向验证已通过）

既有 `WorkPool` checkpoint 只能证明 CREATE 已确认，不能声称覆盖 CREATE 服务端已执行但客户端未知确认的窗口。若继续覆盖真实在途，需要一个局部、每次调用的 CREATE 前 checkpoint。

新增 `cancelled_inflight_create_is_drained_before_schema_cleanup`：独立事务先创建同一随机 schema 并保持未提交；放行原 constructor 的真实 CREATE；用另一 observer 根据精确 SQL、`state='active'`、`wait_event_type='Lock'` 找到 backend PID。取消 ready waiter后，再确认同一 PID 的原 CREATE 仍等待锁，且 `close` 与原 Shared completion 均 pending。

测试随后丢弃这两个清理 waiter、回滚阻塞事务、关闭 blocker pool，再等待原 Shared completion。最后观察 backend 是否退出、schema 是否残留、工作 pool 是否可用、是否进入了取消后的后续 checkpoint，先保存证据并兜底清理，再断言。`close` waiter 的 panic 通过 `catch_unwind` 保留到释放 blocker 之后检查，避免失败阻断释放步骤。

实际锁状态观察各自最多 5 秒，通过 SQL 条件和 yield 等待，不用 sleep 猜阶段。这是验证证据的等待上限，不修改原 25／45 秒执行期限。只有原 CREATE 和 owner 已终结后才 fallback，防止迟到 CREATE 在 DROP 后重新创建 schema。

该 PostgreSQL 同名 namespace 竞争用例已在 Root 的首次 GREEN 运行通过，包括真实锁、同一 backend、清理 waiter pending／取消后重等、backend 退出、无迟到 schema 等断言。无需全局 event trigger、系统目录锁、外部依赖或真实硬件。backend 退出也采用最多 5 秒的原 PID 状态查询等待，避免将 SQLx close 与 PostgreSQL 注销之间的正常异步交接误报为泄漏；该等待未使用 sleep。

## 实际 GREEN 与最终验证状态

本分工已只读核对 `run/validation-20261015-v01-db-cancel/initialization-green.{json,log}`：旧三项初始化失败与新四项取消共 7 通过／0 失败／0 ignored／24 filtered，目标测试 0.62 秒、命令 11.62 秒，退出码 0，默认测试并发，`source_drift=[]`。四项取消回归均在日志逐项显示 `ok`。

`after-green-resources.json` 显示同一任务 PostgreSQL 为 0 schema／0 其他连接。首次 `final-source-manifest.json` 包含 675 项输入；本分工当时重新计算 helper 与独占取消测试文件的 SHA-256，均匹配首次冻结清单。下述修订后需要新的源清单与实际验证，不沿用旧清单声称新源码已通过。

首轮完整 `final-workspace.{json,log}` 显示退出码 101、368.88 秒、`source_drift=[]`；Root 汇总 1265 通过／1 失败／5 既有 ignored／0 filtered，duplex 为 30 通过／1 失败／0 ignored／0 filtered。唯一失败是新 buffered ready 测试在尚未到达 UserSeed 时，就把原 5 秒观察窗口耗在了 admission／初始构造阶段，随后取消并在 `evidence=None` 时提前 unwrap。首次定向只有七项，未暴露此阶段划分问题。该失败属于测试同步缺口，未证明独立 owner 清理失败，也不是历史 Companion V3 根因。

本分工核对源码确认计时确实从 CASES admission 之前开始，失败日志确认未到 UserSeed；日志未记录 admission 的精确起止，因此不把全部 5 秒都归因为已测得的三槽排队时间。`final-resources.json` 在该失败后仍为同一 PostgreSQL 0 schema／0 其他连接。首次失败源码与 binary 已由 Root 归档为 `v1-initialization-cancel.rs.txt`、`v1-duplex-playback`，汇总为 `v1-failure-summary.json`。

修订只改本分工测试：手动注册 ready waker 后，使用既有 UserSeed checkpoint 的 entered／release oneshot 明确同步；确认真实阶段进入并放行后才开始原 5 秒窗口。阶段握手失败不提前 unwrap；先取消并 join 原 owner，若资源身份尚未产生则显式关闭独立 observer，再报告未到阶段。已有资源仍先观察、兜底清理再断言。原 5 秒、25／45 秒期限、`CASES=3` 和默认测试并发不变。修订 rustfmt 后冻结，Root 对实际修订版本直接运行一次完整 workspace 验证，没有增加同源定向重跑。

## 最终 V2 与提交闭环

本分工只读核对 `final-v2-workspace.{json,log}`，并按每个顶层 Cargo target 的最终结果独立重新汇总：1266 通过／0 失败／5 既有 ignored／0 filtered，duplex 31 通过／0 失败／0 ignored／0 filtered。四项取消回归均逐项 `ok`，duplex 用时 46.53 秒，完整命令用时 285.11 秒、退出码 0、默认测试并发、`source_drift=[]`。总数没有重复计入已有子进程测试输出中的内部 filtered 子测试。

`final-v2-resources.json` 在同一 PostgreSQL 测试命令退出约 0.175 秒后记录 0 schema／0 其他连接。`cleanup.json` 记录先核对任务容器完整 ID 与标签，再删除本任务容器及其卷；未改其他容器，共享 runtime 保留。

最终 `final-v2-source-manifest.json` 的 SHA-256 为 `51b11511010fedfa47cf8f01b814eb17731ded45b5404fb7ad01ff2e4ce9c553`，包含 675 项输入。本分工重新计算 helper 与取消测试文件内容摘要，均匹配；`commit-verification.json` 确认源码提交 `8bcda7af892ba36aea98b9de785afa68e3935940` 的 675 项输入全部匹配，没有 mismatch、extra 或 missing input。所有执行与提交均由 Root 完成，本分工未运行测试或数据库。

## 保留边界

本轮证据覆盖当前测试进程与 Tokio runtime 正常存续时的外层取消、未 poll waiter、缓冲 ready 重试、真实 CREATE 锁等待及同一清理 completion 重等。进程被杀、runtime 强制退出或数据库操作永久不返回不在本轮保证内；保留并等待真实数据库 future 不等于给整段生命周期新增一个 45 秒全局上限。

初始 admission／构造仍在原 body 45 秒预算之外；测试阶段门用于定位观察起点，没有扩大业务期限。历史 Companion V3 25 秒超时仍未在本轮复现，不能将这些确认的 fixture 缺口宣称为其根因。未新增依赖、生产逻辑、平台或硬件动作；默认 shadow 和当前政策保持，R03 继续暂停。

## Checklist

- [x] 读取当前 helper、既有初始化失败测试及上一轮 HANDOFF。
- [x] 区分 panic 清理与整个 constructor future drop。
- [x] 设计并实现两个既有 checkpoint 的真实取消 RED 测试。
- [x] 先保存资源证据，再执行 fallback，最后断言。
- [x] 格式化独占测试文件并通知 Root 冻结。
- [x] Root 运行旧实现 RED，归档源快照、失败结果和清理后资源。
- [x] 按新 owner API 适配 GREEN，保留原取消点并显式等待完成。
- [x] 新增真实 CREATE 在途验证，明确等待实际运行证据。
- [x] 核对定向／最终验证结果和资源退出，更新本 HANDOFF。

当前进度：设计与 RED 准备 5／5；运行与最终证据 4／4，合计 9／9。没有生产代码变更，没有硬件动作。本分工已停止修改源码与文档，后续收口由 Root 统一提交文档。
