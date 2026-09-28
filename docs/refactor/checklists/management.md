# R04 · CLI / REST 管理与诊断 checklist

更新：2026-09-28。R01 源码已提交 `a79cb1f`，本包接续已有首轮范围。前端、通用 Embedded 适配及新 Provider/工具平台不在本包引入。

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
