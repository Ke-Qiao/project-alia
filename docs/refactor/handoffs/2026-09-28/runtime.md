# Root / Runtime HANDOFF · 2026-09-28

分支 `codex/architecture-refactor`，本轮起点 `8d74b6c`。DU 11/11 已合入，R01 CP01—CP07 完成，源码已提交 `a79cb1f`。当前没有额度暂停请求；本文件保留连续可恢复进度。

## 实现与责任

- [x] 真实 ServerBuilder 给 Companion 安装 Context / Delivery / Conversation / TTS 与同实例 VoiceInitiativeHandle；API 的 Enabled 只在实际 live 装配存在时接受。默认政策仍 shadow。
- [x] Runner 固定原 opportunity / request / operation / source / 两段期限，最多三次原意图准入，100 ms 间隔且不能越过 admit_before。只由 Fresh permit 进入 actor reserve；Existing 没有重建输出权，退休原 pending/Response，预算不退款。
- [x] reserve 后绑定唯一 Response，规范助手历史原子提交，完整来源/政策重验；只调用一次固定邀请 TTS，再通过原 gate 和 Delivery 投递。Pending/Sending/Observing 不被误记为成功，只有 Submitted/Completed 才解除 runner 取消责任。
- [x] 每 channel 一次、实例同时最多四次 live producer；JoinSet 归还 channel 槽。已尝试与已检查候选分别有界跟踪，SQL LIMIT 前排除，缺少机会可再评，失败后不换新机会续权。shadow 批次最多 5 秒，避免阻塞 live 扫描。
- [x] 所有等待受原候选期限限制。关闭先取消并 join 全部 live owner，再关闭依赖；原公开 TaskTracker 排空继续保留。panic 关闭 admission，不抛弃后台任务。
- [x] 修复示例 `embedded.example.toml` 重复空 identity table，新增通过真实配置 loader 读取发布样例的回归。

`admit_before` 仅限制准入提交与首次 reserve；`valid_until` 是原 candidate deadline，贯穿输出/Hold/恢复。不得把机会两秒新鲜度改成完整音频生命周期期限，也不得靠继续/重试延长它。

## 已有证据与正在执行

- [x] 定向真实 PG/Context/Runner 38、RuntimeHTTP 7；组件 50；最终完整 duplex 9。它们与 workspace 有重叠，不累加。
- [x] 冻结 V1 630 输入，SHA-256 `58c76e82adf71fed96fa6b113f51185412dd3bd0619a3db747835f3d81accd4a`；默认 workspace 1036/0/5，493.98 秒，source drift 为空。
- [x] macOS Server / Embedded no-default all-targets 均通过。
- [x] 真实 PTY 三项、实际 Easy-Turn 两项、Linux ARM64 两端编译均通过，源码无漂移，合计 1041 unique macOS；结果见 [验收摘要](validation-summary.json)。
- [x] 正常本地提交 `a79cb1f`，630 个已提交输入与 V1 一致。

运行包装器检查全部冻结文件的前后哈希及新增输入；docs/run 不属于编译输入。源码冻结后任何修复都必须另建 manifest 版本，不能覆盖 V1。计数脚本只统计每个 Cargo 目标最后的父 summary，故意启动的嵌套 crash 子测试不另加。

开发期失败保留：初始导入/TransactionTrait 编译错误；删除一个源消息使所有包含该历史根的后继候选也被擦除，修正测试错误预期而非削弱擦除；paired 测试先补真实设备绑定/握手，随后处理示例重复 identity。V1 全量没有失败。

## 资源与边界

- 本轮重新启动 OrbStack 后，旧任务 PG 容器实际不存在，创建了独立 `alia-refactor-test-db`，只监听 `127.0.0.1:32768`。用户 `alia-postgres:5433` 未操作；不能沿用上轮“三个残留 schema”的旧叙述。
- 实际 Easy-Turn 已从本地已校验源码/权重恢复在 8091：CPU float32，模型 `7d1f8b0` / 源码 `fe175eaa` / Qwen `7ae5576`。没有下载或升级依赖。
- Linux 验证容器 `alia-cp-arm64-20260928-v1` 使用隔离只读 V1 快照与克隆缓存；仅编译，不冒充 Linux 运行/声学验收。旧工作树、缓存、证据均保留。
- 真实 PG 提交 barrier 用例不等于真实网络丢失 PostgreSQL commit ACK。配对 H2 的 Resume ACK 丢失是实际测试中的受控注入。
- 当前邀请 Context 不用于普通 Generation 历史输入；这一只读历史扩展已明确记入延期台账，不扩大邀请的授权用途。
- Pi 系统/音频硬件/执行器信息仍未确定；R02/R03/R05 真机及 24 小时验收开放。前端、通用 Embedded 适配、Pi 运行 Easy-Turn 不进入当前工作。

## 紧接下一步

1. CP07 已完成：`a79cb1f`、630 输入一致；所有证据已归档。
2. 从 [R04 只读核对](r04-readiness.md) 的三个已定位缺口继续：HTTP 新装完整配置、Companion OpenAPI、OpenAI 在线健康探测超时。R01 提交前不改这些源码。
3. 继续用三个 subagent，各占一个独立模块；Cargo/PG 统一串行窗口。Root 做真实运行时组合验证和正常提交。
4. 可先进行无需硬件的软件工作，等待用户补充 H01—H05；不能把硬件前置未到位当作软件 R01 的失败或完成依据。
