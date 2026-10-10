# Interaction Response transaction owner：L11

基线 `f15db66`，分支 `codex/architecture-refactor`。本分工实现与静态检查完成 **7/7**；最终同源 Cargo/PostgreSQL 验证由 Root 统一接手。未提交、未运行 Cargo/数据库/容器/服务，未访问 5433 或凭证。

## 已实施契约

`interaction::response::ResponseCommitter::new(DatabaseConnection)` 是具体应用事务所有者。`commit_response(owner, session, response, ChatMessage)` 保留原角色/name 验证，持有一个数据库事务，参与 Context authority 复验、Delivery Response 与 audit 更新、canonical assistant history 插入/核对、session 更新时间。原 2 秒 ACK 不确定性超时与历史重试返回相同 UUID、擦除后不重建 history 的行为均保留。

ConversationStore/ConversationService 删除 `commit_response`，普通历史端口不再承担跨域提交。InteractionServices 和 Companion LiveOutputs 使用 `responses: Arc<ResponseCommitter>`；两个生产调用者 persistence / live runner 共用同一契约。Root 负责 Bootstrap 与 ModuleRegistry 的同名装配字段。Delivery 的事务参与者改为不带连接的关联函数，由 Root 实施，ResponseCommitter 只传当前 tx。

此处 Context 是不可变已物化证据及同事务锁复验，没有新增 `frozen_at`、`output_closed_at` 等列，也没有提前 validate 后把结果当写许可。默认政策、Shadow、输出身份与 Delivery 展示事实均未改变。

## Fixture 与回归

- 删除 integration fake 自行实现的一份不完整 Response/history SQL；linked fixture、cache origin seed 使用实际 ResponseCommitter，assistant history 断言改查 SQL。
- 保留不应进入输出路径的 Companion 失败 fixture，使用断开的 ResponseCommitter，意外输出仍会失败。
- 扩展 `canonical_response_and_history_commit_once_or_roll_back_together`：分别注入 assistant INSERT 与最终 session UPDATE 故障，确认错误含注入标记；Response pending、history/audit 均为零、Context 完整记录及 session 更新时间保持。并发、内容冲突、owner/session、role/name 和无设备展示断言保留/加强。
- 新增 `committed_response_retry_never_recreates_erased_assistant_history`：删除已提交 assistant 后由另一事务 owner 实例重试，返回旧 UUID，history 仍空、audit 仅一条，篡改重试失败。
- 新增 `response_commit_failure_suppresses_completion_and_memory_capture`：真实 pipeline + SQL 触发器故障，非事务 sequence 的 is_called 证明实际到达注入点，Response pending 1 / committed 0，无 assistant、ConversationTurnCompleted 或 memory_capture trace。
- 旧 fake failure 循环事实上在缺失 source 检查就返回；保留并准确重命名为 `missing_source_does_not_publish_completion_or_memory_capture`，真实提交失败由新增端到端测试覆盖。

## Checklist

- [x] 追踪全部生产调用者、Conversation trait/实现、fixture 和事务参与者。
- [x] 迁移具体 ResponseCommitter，删除旧普通历史端口的提交职责。
- [x] 迁移普通响应、Companion 邀请与相关构造者。
- [x] linked fixture 使用真实提交实现，保留 SQL 回归断言。
- [x] 补充末写失败回滚、擦除重试、失败后不发布完成/记忆捕获。
- [x] scoped rustfmt、git diff --check、事务体静态等价比较通过。
- [x] Root 在冻结源码上完成实际运行验证并记录结果。

静态记录：`run/l11-response-20261010/static-audit.json`。与 `f15db66` 的原事务方法比较，除 `self.db()?` → 显式连接、receiver → 关联事务参与者及可见性外，删除空白后的方法体完全相同。该比较证明本分工未改事务顺序或业务条件，不代替 PostgreSQL 并发/回滚测试。

Root 收尾需确认新增 `response.rs` 纳入源码提交，并统一归档静态记录。此交接后的全部文件可冻结，由 Root 接手；本分工等待验证结果，未经通知不继续编辑。

## Root 最终验收

源码 `60f665cf6fce4811b144dfc743b4814c5bdf19d5`，V4 manifest `520ec684c2619e775389330adbe837113edcb675e2f7d62fc7f5d65758fe274f`，640/640 Git blob 一致。macOS workspace **1060/0/5**、真实 PTY **3/0**、无默认 availability **2/0**、提前准入 **1/0**；Linux ARM64 Server 完整默认并行复跑 **778/0/5**，两端无默认全目标及 Linux release/native link 通过。所有最终结果同源且无漂移。

本轮 10/10，L09—L12 3/4，R01—R06 仍 2/6。四个任务容器均停止。停机前 schema/其他连接均为零，无需额外删除 schema。原失败、修正和最终证据见本目录 evidence-index.json。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械与其他平台。Linux V4 首次旧 panic-fence 子进程 20 秒超时仍保留为未定位项；定向和完整复跑通过不等于根因修复。
