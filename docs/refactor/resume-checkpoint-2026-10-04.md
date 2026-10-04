# 2026-10-04 恢复入口

本次用户授权范围已收口：交付证据闭环、V01 有界调查、当前 Linux ARM64 默认部署构建/运行、R02/R03 软件前置。不是新一轮整体重写。分支 `codex/architecture-refactor`；开始 HEAD `8c17422`，本轮源码提交 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23`。四份当前 HANDOFF 见 [总入口](handoffs/2026-10-04/README.md)，最终文档作为后续独立记录提交；旧日期检查点保持历史原样。

## 进度与当前冻结

- DU **11/11**；R01 **CP7/7**；R04 原软件包 **4/4 + MG05**。
- 六轮 **2/6**，已完成 R01/R04；R02/R03/R05/R06 整轮仍开放，不用本次前置项代替实机验收。
- 本轮 source v1：**633 输入**，SHA `a1c7dc3d6787cf2b86144b60c17b5aaaaad65025d1695a1326194c826c8ba0d0`。冻结后不并行编辑源码。
- macOS `cargo test --workspace --locked --offline`：**1062 passed / 0 failed / 5 ignored**，541.76 秒，drift=[]。
- 真实 PTY **3** 已通过并保留 transcript；本轮不同 macOS 通过数 **1065**。新诊断 binary 的 paired 9/9 不重复累计；两个真实模型用例本轮没有执行。
- Linux ARM64 两端默认 release/link、实际两轮 PG/HTTP/gRPC 启停和重启均通过；8 个目标 **578/0/3**，默认测试并行。3 个 Linux PTY ignored 没有执行，两个模型 targets 没有选入；不与 macOS 相加。
- 源码提交与全部冻结输入匹配，额外/缺失/变化均为 0，见 [逐输入核对](handoffs/2026-10-04/committed-source-verification.json)。三个 subagent 均已完成，Cargo/PG 窗口空闲。

## 本轮已完成

- [x] 真实 Git/清单核对：R01/R04 commit 输入一致，R04 记录提交只改文档；原证据 51+36 份哈希匹配。
- [x] 原 R04 1053/0/5 与 nested test 去重核对，5 个 ignored 逐项原因/依赖/边界已记录。
- [x] 保留 V4 paired binary 三批默认并行 9/9；**原 V3 failing artifact 未核实保留，V01 继续 open**。没有扩期限、串行化或刷新过期机会。
- [x] 原生 capture continuity/stream 变化会退休旧会话；KWS reset 清除跨缺口部分唤醒词，3 项回归通过。
- [x] 真实 DeviceBinding/Body endpoint 的未绑定/其它 controller/重连与旧 boot 拒绝；mock writer 和既有 pin/ACK/ledger 回归通过。
- [x] setup HTTP/CLI 单次完成，取消后拒绝迟到 body，Host/Origin/JSON 入口防护；保留业务失败 HTTP 200 + success=false。
- [x] health 顺序/重复探测与消费者、配置目录 fsync 缺口已核对；未新增框架或更改 probe 默认值。
- [x] 当前 SSH 本机上下文不足以确认 Pi 目标；保留历史信息与未知硬件表，没有 SSH 扫网/录放音/真实动作。
- [x] 完成 Linux 默认 release/link/run 的最终矩阵，保存实际 feature、工具链、native 依赖和二进制身份。
- [x] 正常本地源码提交、逐输入核对、任务资源停止；当前四份 HANDOFF、机器状态、证据索引和后续 checklist 已写好。

## 保持的策略与资源约束

默认 shadow，不擅自扩大无人值守主动投递；保留原 output owner、临时避让/明确暂停、来源撤销、预算、CAS、unknown 与原重试身份。Easy-Turn 留 Server；无新依赖/前端/R06 平台扩张。Cargo/PG 同时仅一个窗口。

仅本轮新增 `alia-review-20261004-db`、`alia-review-linux-20261004-v1`，**现均已停止**。两轮实际 release 的四个进程均 SIGTERM exit 0、0 强杀、0 遗留监听；Linux idle holder 随容器显式停止的 exit 137 不是编译/测试失败，OOMKilled=false。自有 smoke schema 已删除，Root 最终确认任务非 public schema/其它连接均为 0。

旧 `alia-refactor-test-db` 保持 stopped，用户 `alia-postgres:5433` 保持 running，未查询或修改用户数据；旧验证容器/快照/失败日志保留。本轮无真实录放音、SSH、绑定更改或动作，也没有推送/PR。保留模型服务未探测或修改，当前运行状态不作断言。见 [资源原件](handoffs/2026-10-04/resource-cleanup.json) 和 [证据索引](handoffs/2026-10-04/runtime-evidence-index.json)。

## 后续任务顺序

1. 先读本检查点、[机器状态](resume-state-2026-10-04.json)、四份 HANDOFF 与实际 Git 状态；确认没有尚在运行的唯一验证窗口。
2. 本轮已完成，没有需接回的编译/测试任务。后续文档变更不重跑已通过矩阵；若源码再变，另建冻结版本和相应验证，保留本包原件。
3. R02/R03 先取得当前可信 Pi SSH host/alias 及音频/控制板/执行器资料；只读采集 OS/toolchain/ALSA/USB/串口/资源状态。信息缺失继续记 unknown，不猜旧 IP/硬件型号/安全姿态。
4. R02 接入成熟 AEC 前固定参考消费、10 ms 重分帧、独立时钟/漂移、同步测量与素材保留；实际单讲/双讲/尾音和长期运行另验。
5. R03 先明确资产内容签名与控制板 boot/ACK/反馈/限位协议，再在有人监督、低风险工况验证停止与恢复；mock 通过不关闭 R03。
6. R04 增强继续沿原 checklist 处理整体 health 预算/重复探测，以及目录持久性和部署账户权限；不搭新平台。
7. V01 保持 open，复现时保留原二进制、fixed phase、ScanProgress、retirement 与安全预算/等待字段；禁止无限重跑。没有根因证据不得自动将 shadow 改 Enabled。
8. R05 干净安装/配对/升级恢复与 ≥24h 实机混合负载；R06 仅在指定其它 OS/CPU 后启动。前端、通用 Embedded 和同机/端侧模型产品化保持原延期。逐项状态沿用 [R01—R06 checklist](checklists/next-round.md)，不能因当前软件前置关闭整轮。
