# 2026-09-26 R01 基础包交接与验证

DU11 已合入主重构分支。R01 当前完成 **CP01—CP04：4/7**；CP05 真实主动投递、CP06 live 故障矩阵及 CP07 完整 R01 验收仍开放。产品默认 shadow，生产 actor 和 API 均拒绝 live 输出。

## 四份 HANDOFF

- [Root / Runtime](runtime.md)：装配、停机、HTTP、组合验收及下一包分工。
- [Policy](policy.md)：11 项纯策略、回放、TTS HTTP 夹具修复。
- [Repository](repository.md)：9 项真实 PG、来源闭包、擦除、容量、持久去重。
- [Voice / Embedded](voice.md)：机会快照、snapshot-only actor、显式协议及 Linux ARM64 编译。

## 统一证据

- [验收摘要](validation-summary.json)、[V3 输入](final-source-manifest-v3.json)、[全量结果](workspace-v3-result.json)、[全量去重计数](workspace-v3-counts.json)。
- [V1 原失败](workspace-v1-result.json)、[V1 计数](workspace-v1-counts.json)、[V2 导入编译失败](companion-v2-result.json) 均保留；它们不被 V3 覆写，也不与 V3 叠加。
- [Companion 定向](companion-v3-result.json)、[TTS HTTP 定向](tts-http-v3-result.json)、[实际模型](models-v3-result.json)、[真实 PTY](pty-v3-result.json)。
- [Linux ARM64 编译](linux-arm64-v3-result-summary.json)、[macOS Server](server-no-default-v3-result.json)、[macOS Embedded](embedded-no-default-v3-result.json)。

V3：619 输入；manifest SHA256 `8ea6d4f7cc63b82d22b8c584c8c0b426b11e04f952f64bd15b7776d352cc18a1`。macOS 完整 workspace **1005 passed / 0 failed / 5 ignored**；另行真实 PTY 3、实际 Easy-Turn 2 通过，合计 **1010 项不同测试**。定向 26、TTS HTTP 9 已包含于全量，不再次累加。本轮 Linux 只做真实 native 编译检查，没有运行 Linux 测试。

原始日志和验证脚本位于 `run/validation-20260926/`；测试计数脚本按每个 Cargo 目标最后的父 summary 计数，排除故意崩溃测试的嵌套子进程结果。两个任务专用 Linux 容器已停止，源码快照、镜像、缓存和证据保留。

下一步从 [CP05 总体设计](../../companion-live-design.md)、[存储契约](../../companion-live-storage-design.md)、[设备契约](../../companion-live-device-design.md) 继续。三份均是待实施设计，不能作为 live 或硬件验收证据。
