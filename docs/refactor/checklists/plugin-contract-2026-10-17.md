# 插件契约与信任边界 checklist

> 2026-10-19 实施更新：PN01—PN04 的当前软件范围已交付，Rig／MCP／Extism 与真实 Rust PDK guest 已完成 macOS 回归，见[软件发布记录](../../operations/software-release.md)。当前待办统一见 [Project TODO](../../todo.md)。下方 PC01—PC10 保留 10 月 17 日的设计验收记录，不能将其 10/10 换算为全部 PA 场景或 Pi 实机已通过。

2026-10-17；起点`89d04a2`，应用源码仍`6c62c84`。本轮是契约定稿与资料清理，不交付插件运行器或隔离后端。[规范契约](../../architecture/plugin-contract.md)、[四份HANDOFF](../handoffs/2026-10-17-plugin-contract/README.md)、[恢复检查点](../resume-checkpoint-plugin-contract-2026-10-17.md)。

## 本轮进度：10/10

- [x] PC01 核对起点、既有证据和范围；默认shadow、当前政策／执行批准、R03暂停及原期限／并发保持。
- [x] PC02 三个subagent分别核对Server授权／存储／Execution、Embedded包与进程、表达ABI与独立审查；没有把静态发现写成动态复现。
- [x] PC03 区分受信编译扩展、外部服务、本地进程、未审核代码与Skills，固定核心所有权。
- [x] PC04 分开来源、内容身份、能力授予、运行隔离和业务批准；明确用户要求的未审核代码安装及后续隔离运行目标。
- [x] PC05 明确资产／实现／授权／实例／命令身份与版本兼容；复用原Execution身份，升级不复用旧批准。
- [x] PC06 明确暂存发布、握手、准入、取消、原owner完成、unknown、迟到事实及卸载数据保留；升级排空与撤权取消按已确认D03固定。
- [x] PC07 清理旧概念和包文档中的过期ModuleContext／Pipeline、插件框架与Body内容签名描述，链接统一契约。
- [x] PC08 映射实际调用点、已有回归与PA01—PA11后续验收；不创建未选择的SDK／MCP／隔离框架。
- [x] PC09 用户确认管理员管理＋用户隔离、默认隔离＋独立受信本机模式、升级排空／停用撤权取消；三份分工按决定冻结。
- [x] PC10 完成文档链接／diff／未改运行输入核对，归档静态证据，更新四份HANDOFF和恢复入口；草案`826dbf0`与确认后的定稿分别正常提交，保留两阶段证据。

上述10项只计当时的设计交付，不作为项目完成百分比。下方实施状态依据后续代码与软件验证更新，PA 和平台结论须按实际支持面读取。

## 实施阶段：当前软件范围已完成

下表保留各阶段的原始目标；当前落地方式以 [WASM 插件](../../operations/wasm-plugins.md)、[MCP](../../operations/mcp-tools.md)及下方状态为准，旧子进程表达 ABI 不再提供。

| 阶段 | 范围与复用 | 完成条件 |
| --- | --- | --- |
| PN01 Server管理与数据边界 | 先复现全插件export/purge的普通AuthUser边界；复用当前管理员身份复验、原存储／CAS／TTL／retention | 普通用户／降权／失活拒绝且无数据改变；本人scope不靠自报owner。对应PA01，保留Execution owner批准 |
| PN02 Embedded资产与生命周期 | 先复现包名／版本／原地发布、Starting/Stopping与旧wait回写缺口；同时更新proto两端和Server逻辑command ID | 同次有界资产验证、不可变发布、实例owner／原完成句柄、稳定命令与流语义、原期限并发。对应PA02—PA08中的包／进程场景 |
| PN03 表达ABI及外部工具接线 | 先复现输出预算／UTF-8／原子进程清理差距；接原Appraisal pin、Context和Execution | 最小私有输入、内容／环境身份和旧批准失效，原业务政策／内建安全校验保持。对应PA04、PA06、PA09 |
| PN04 隔离后端与第三方开发验收 | 按当前Mac／Pi范围选择实际可验证后端，另行确认必要依赖；将已验证接口整理为开发文档与示例 | 一个受信示例及一个未经审核示例完成安装、授权、运行、升级、撤权、卸载；真实隔离负例通过。对应PA10／PA11 |

- [x] PN01：真实RED 0/2后修复管理／本人scope与事务身份复验；HTTP 7/0、storage 4/0，原数据保持和清理证据见[PN01检查点](../resume-checkpoint-pn01-2026-10-17.md)。
- [x] PN02：有界资产校验／发布、内容与实例身份、稳定命令、原 owner 排空和 unknown 语义已实现；WASM 与显式 trusted_native 分开准入，清理未完成保留 owner／资产。
- [x] PN03：工具构建和 MCP 已接 Execution；表达迁为 WASM ABI 3，最小私有输入、实现 pin 批准、撤权及最终内建安全校验已回归。旧表达子进程与 ToolExecutor 已删除。
- [x] PN04：Extism 有界 WASM 及 Rust PDK 示例已实现并做真实 guest 回归，覆盖安装／授权／升级失效／撤权与隔离限制。仅声明当前计算型工具、表达和 processor 支持面；无通用 native 沙箱、存储 host broker 或 Pi 实机容量结论。

PA 编号是目标行为映射，不是已全绿的计数表；每次扩大权限、语言或平台支持时复用已有测试，仅补相应缺口。真实模型链按用户决定暂缓，输入沿用[前置检查点](../resume-checkpoint-model-preflight-2026-10-17.md)；R02／R03／R05 现场门槛不变。

历史优先级：用户 2026-10-17 要求先做首个 R02 USB 接入／枚举，已完成只读核对且未发现 R1。随后用户授权并完成上述 Harness 重构；这不自动恢复 R03，也不扩大真实录放音范围。
