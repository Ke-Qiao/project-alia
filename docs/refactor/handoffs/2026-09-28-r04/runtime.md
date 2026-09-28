# Root / Runtime HANDOFF · R04 · 2026-09-28

承接 R01 源码 `a79cb1f`、记录 `75bf97c`。本包实现4组管理修复，633输入V4完整软件验收已完成，正常本地源码提交 `8f5bb83`，已提交输入逐项相同；没有更改用户设备政策。

- [x] 三名 subagent 分别完成 HTTP 配置、Companion OpenAPI、Provider 健康与设备管理；Root统一端到端调用核对与验证。
- [x] Root修正未认证setup写入入口：仅loopback，无permissive CORS/前端attach/LAN广播；远程使用SSH隧道，生产Server正常discovery不变。
- [x] 复用既有HttpListener/Connections；ConfigReady后保留原server future，graceful排空成功响应，2秒后关闭实际stalled socket并join。Drop封住连接，CLI输入也继续join。
- [x] 真实loopback测试验证listener地址，以及完整生成→保存成功响应、磁盘加载、未完成body的关闭与listener释放。
- [x] 无前端操作指南核对配置、绑定、政策、记忆、人格、执行确认、Delivery证据及诊断；不引入Temporal/Rig/rmcp/Home Assistant新依赖。
- [x] V4 macOS workspace 1053/0/5（319.39秒）、真实PTY3、实际Easy-Turn2全部通过，1058 unique；两端no-default all-targets与fmt通过，源码无漂移。
- [x] Linux ARM64两端no-default all-targets编译通过；源码提交 `8f5bb83`，633个已提交输入与V4逐项一致，证据归档完成。

## 验证身份与失败保留

- V1：633输入 `7e6adbd3ed00b47a572b9754e33d3b4ee5e23628a454a9ee72a7342e20b99025`；lib编译失败，ApiDoc测试误引用未声明tower；未执行测试。移除框架重复测试，非法UUID断言放入既有真实HTTP用例，没有新依赖；同时移除未使用的frontend re-export。
- V2：633输入 `8994cf93c68e3fe6aa81c767b6541eafd4e0889b389286bad9e901e2e026b28f`；lib308/1/3。唯一失败TCPrefused fixture 2秒超时，单项同源复核也失败。不能断言已证明端口复用或某个系统策略是根因。
- V3：633输入 `b3ff3f99531932bd169d9c73b92cc16d7d6b341e655d37e68b972df3e913897a`；只把Unix平台refused fixture改真实Unix-domain socket，先验证OS ConnectionRefused，再执行同adapter分类。生产5秒/测试2秒和全部TCP状态/stalled路径未改。V3完整workspace在第27目标停止，699/3/3；旧3个Companion配对场景在25秒内未结束。同原workspace binary随后9项默认并行全部通过（13.69秒），根因仍未确认。

- V4：633输入 `35efd1807b5c2820b3024fc6142bf769ace5973cc13275e0b5649f8bec011a8b`；仅在旧配对测试加入固定阶段以及最多1秒、无内容/身份的失败状态摘要，25秒与生产期限不变。完整workspace已通过；初次V3超时根因仍未确认，保留原失败和新增诊断。

所有原始记录在 `run/validation-20260928-r04/`，统一包装器记录命令、退出码、源码前后哈希和新增文件漂移。测试数按每个Cargo目标父summary统计，不叠加定向或嵌套子进程运行。

## 边界与资源

任务PostgreSQL在32768，用户5433未操作；实际Easy-Turn在8091保留。R04 Linux V1仅创建，未复制源码/未执行Cargo；V2复制633输入并通过host/container hash，但未执行Cargo，随后停止idle容器。它们不代表Linux编译失败。V3没有Linux快照；V4独立快照编译通过，容器已停止，前后源码和Cargo.lock一致，历史证据保留。最终PG核对非public schema为0、其他连接为0。

设备HTTP fixture验证当前DB owner/admin授权及拒绝先于action、真实adapter结果映射。公开GetBindingStatus只说明本次RPC可达，不证明设备可信body pin、绑定正确、CPU/温度遥测或真机声学；未知遥测保持null。健康探测5秒是每次请求，顺序聚合多个probe耗时可能累加。

Pi系统/声学设备/执行器与最终部署清单仍待提供，R02/R03/R05保持开放。Server其他目标和未来设备侧模型/同机部署按R06另行指定和实测。没有把这些信息缺口改写为软件失败或完成。

最终源码：`8f5bb83f4a313723428abf854c1e663b159c6c45`；[已提交输入核对](committed-source-verification.json)、[整包验收](validation-summary.json)。后续从 [检查点](../../resume-checkpoint-2026-09-28.md) 和 R02/R03/R05/R06 checklist 接续。

停止后的Linux容器exit 137来自idle holder在docker stop后的退出；Cargo Server与Embedded均为exit 0，OOMKilled=false，不把容器终止码混为编译结果。
