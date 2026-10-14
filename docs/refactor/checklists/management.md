# R04 · CLI / REST 管理与诊断 checklist

更新：2026-10-11（health 与配置持久性续轮已收口）。原 R04 包源码 `8f5bb83`，其历史结果保留在下方；本次补充审查见 [2026-10-04 交接](../handoffs/2026-10-04/README.md)。前端、通用 Embedded 适配及新 Provider/工具平台不在本包引入。

当前四个已定位缺口 **4/4 完成**，MG05统一验收及源码提交 `8f5bb83` 已完成；该数字表示本包的四组修复，不是整个项目百分比。原始只读依据见 [R04 readiness](../handoffs/2026-09-28/r04-readiness.md)。

- [x] **MG01 HTTP 新装配置**：完整样例及其 profiles/providers/policies；未认证 setup 仅监听 loopback、无跨域/局域网广播；真实配置加载与语义检查成功后才发 ConfigReady。坏配置/写入失败不得误报成功；验证独立目录和实际 HTTP 路由。
- [x] **MG02 Companion REST 文档**：四个现有操作的请求/响应、Bearer owner、CAS/410/503原身份语义进入 OpenAPI；序列化契约与真实错误形状核对；给出终端操作步骤。
- [x] **MG03 Provider 诊断有界完成**：实际 OpenAiLlm 在线 probe 有限期限；真实本地 stalled response 被观察后，完整 `/health` 返回降级及其他模块状态。401/404/refused/timeout 可区分，默认 probe=false。
- [x] **MG04 设备授权、状态与操作核对**：统一 owner/current-admin 门覆盖设备读写，缺省多渠道要求明确选择；status 使用实际只读 RPC、遥测缺失保持 null，失败不伪造 offline。真实 HTTP 回归后再完成操作文档。

操作用例已核对：配置、绑定、政策/记忆、人格、执行确认与诊断均有现有 CLI/REST 入口和对应文档；明确权限与 unknown 处理，不依赖前端。

- [x] **MG05 组合验证与提交**：统一输入、必要定向及整合回归、平台影响、正常本地提交与可恢复数字。

分工：`companion_replay_28` MG01；`companion_policy_28` MG02；`companion_delivery_28` MG03 与设备路径；Root 操作用例与 MG05。共享源码，仅按文件分工；Cargo/PG 同一时刻一个窗口。

C01 Temporal 与 C02 Rig/rmcp/Home Assistant 的候选门槛不因被提案点名而自动满足。本包修复已有实现，不增加新外部服务或依赖；后续具体用例、版本/权限/取消/恢复验证仍按 [台账](../deferred.md) 记录。


## 验证记录

- V1：633 输入，Server lib 因 ApiDoc 测试引用未直接依赖的 `tower` 而编译失败，0 项测试执行。移除框架重复测试，将非法 UUID 断言放入既有真实 HTTP 用例；没有新增依赖。
- V2：633 输入，Server lib 308 passed / 1 failed / 3 ignored。8 项 OOBE、2 项新增 setup、ApiDoc 和 HTTP 状态测试通过；唯一失败是 TCP 连接拒绝夹具外层 2 秒超时，原源码单独复核也失败。
- 修复方向：只把 Unix 平台拒绝夹具改为真实 Unix-domain socket，持有已 bind 未 listen 的地址，先断言 OS ConnectionRefused，再走原 OpenAiLlm error 分类。生产 5 秒与测试 2 秒不变。该证据不声称验证 Windows/TCP refused；实际 TCP 200/401/404 和 stalled response 仍单独验证。
- V1/V2 的 Linux Cargo 均未执行，容器/快照仅准备阶段；不能把 macOS 失败归为 Linux 失败。
- V3 全量在第27个目标停止：699 passed / 3 failed / 3 ignored；3 项旧 Companion 配对场景在原25秒期限内超时，新增 Server lib 全部通过。同一 workspace 二进制随后默认9项并行全部通过（13.69秒），根因未确认；包内单例通过使用了不同二进制，不能混为同一证据。
- V4只补固定阶段与有限只读状态诊断，保留所有期限。完整workspace **1053/0/5**（319.39秒），真实PTY3、实际Easy-Turn2，合计 **1058 unique macOS**；两端macOS/Linux ARM64 no-default all-targets与fmt通过。Linux仅编译。
- 源码提交 `8f5bb83`，633输入与冻结V4逐项一致，无额外/缺失输入。首次V3配对超时根因仍未确定，按台账V01继续保留，不宣称由诊断改动修复。见 [四份交接与验收摘要](../handoffs/2026-09-28-r04/README.md)。

后续入口：R02/R03/R05需要H01—H05真机/部署信息，R06需要明确OS/CPU/runner；不因本包软件通过关闭这些出口。C01/C02尚无本包必须替换的已验证用例，因此没有引入新组件；启动门槛仍见台账。

## 2026-10-04 有界补充审查

- [x] 核对 R01/R04 提交与输入、原始日志、去重口径和 5 个 ignored 的具体环境；历史通过与当前执行分开记录。
- [x] 先查 setup 的并发、Host/Origin/JSON、文件权限与持久性；补 HTTP/CLI 单赢家和取消期间拒写，保留 JSON 保存失败契约。本轮实际矩阵与新提交见 [验收摘要](../handoffs/2026-10-04/validation-summary.json)。
- [x] 核对公开 device status 没有升级成输出授权，补真实 DeviceBinding/Body gRPC 的未绑定、其它 controller、新连接和替换 boot 拒绝场景；驱动仍为 mock。
- [x] 核对 `/health` 消费者、顺序聚合、router 重复 minor probe 与 lock、默认 probe=false；未新增健康框架或付费探测。
- [x] **整体 health 限额**：2026-10-11 源码 `29420a5`，保留 API/default probe=false，6 秒总预算／8 并发／同 Registry 单轮准入，相同 profile 共享实例，Router 本地检查，Gemini 5 秒有界与脱敏；无缓存或新增诊断入口。实际同源验收见 [本轮 10/10](health-2026-10-11.md)。
- [x] **配置目录持久性软件闭环**：源码 `0954996`，base/fragments/目录树与完整 canonical 父链同步，未知发布保留文件与 descriptor，同意图重试及 loader 恢复；正常加载保持只读。隔离文件系统双平台验证见 [本轮 10/10](config-durability-2026-10-11.md)。真实断电与部署文件系统保证不由软件测试代替。
- [ ] **可信本机文件系统边界**：Host/Origin 防护不认证同机进程；操作者路径、服务账户和权限需要在实际部署固定。单进程门不冻结其它进程修改 fragments/symlink 的行为。

后面三项是 2026-10-04 审查记录的 R04 增强/部署出口，其中 health 与配置持久性软件闭环已在 2026-10-11 分别验收，可信本机部署边界继续开放，不回写为原 MG01—MG05 已覆盖，也不自动启动新框架。V01 仍开放，见 [台账](../deferred.md)。

## 运行时 Settings 恢复续轮

- [x] 源码 `2f5e2a2`，原子文件保存、明确发布失败阶段及显式重试；privacy 按用户确认统一 SQL 政策与管理员修改，保留成功响应形状与原读取权限。见 [本轮 10/10](settings-recovery-2026-10-11.md)。
- [x] 纯非政策 PATCH 等待后身份核验已完成，源码 `4553e88`，见 [本轮10/10](settings-authorization-2026-10-12.md)。

## 2026-10-12 Settings 身份重验证后续

- [x] GET Settings 等待后敏感响应准入已完成，源码 `d080710`，见 [本轮10/10](settings-read-authorization-2026-10-12.md)。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。

## 2026-10-12 Settings GET 后续

- [x] Settings 锁等待与取消有界审计完成，新增两项局部取消回归，见 [本轮记录](settings-hardware-2026-10-14.md)。这不表示全部等待已有期限。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。

## 2026-10-14 主线切换

主要投入转为 R02-A 双流证据/离线重放、R03-A 受信动作资产内容认证，之后按硬件资料接入真实AEC/反馈，再进入R05。H01—H05待当前部署确认，R02/R03/R05保持开放。

- [x] 2026-10-15有界软件故障包：慢模型持锁、SoVITS权重额度期限已修复；真实HTTP/1 handler取消、同值重试与privacy共享锁关闭已验证，见[本轮10/10](settings-waits-2026-10-15.md)。已提交SQL/desired/module部分成功保持，未增加整请求timeout或全局回滚。
- [ ] 整体Settings没有统一SLA；同步文件I/O、任意扩展和真实模型/部署取消仍需对应验证。纯GET/PATCH update gate排队未单独取得HTTP阶段见证，不能把局部Registry证据推广成全协议保证。

## 2026-10-15 Settings 等待与关闭收口

源码`4e99134`。Server全目标868/0/5通过（真实PTY3、Easy-Turn2未运行），随后仅cfg格式调整，最终provider51/0/0及无默认all-targets检查通过。669个最终输入逐blob匹配提交；精确格式转换、原RED、基线关闭PASS及实际清理证据见[四份HANDOFF](../handoffs/2026-10-15-settings-waits/README.md)。本轮独立数据库已删除，共享运行时保留；R03暂停和R05现场门槛不变。
