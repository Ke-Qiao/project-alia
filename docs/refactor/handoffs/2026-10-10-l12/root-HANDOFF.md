# L12 Root HANDOFF：完成

分支 `codex/architecture-refactor`，基线 `d5481b5cf4a6eeb28f535f08a8b1b9be7b31d5c9`。L12 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**；这些是条目计数，不代表整个项目完成百分比。三个 subagent 已交接，Root 完成集成、验证和正常提交。

## 完成的职责

Delivery 拥有 `DeliveryTransport`、`DeliveryTarget`、音频/文本 payload、observer/report、精确取消及输出控制端口；Channel 保留入口、能力、生命周期与 generation/session fence，具体 CLI/Embedded 直接实现出站端口。原 Arc upcast、原 producer 与完整 Channel 捆绑校验保持真实资源实例；同名替换回归扩展了既有测试。

跨 TTS/Channel/HTTP 的字节预算归 `delivery::audio`，provider stream alias 与 GPT-SoVITS 私有累计上限留在 provider；原限额、等待取消和 RAII 最后引用释放算法不变。旧 `ChannelTarget`、Channel 出站定义及 TTS budget 文件/出口删除，架构守卫防回流，无兼容包装。

三个分工分别是 outbound、budget、playback timeout；Root 独占 Cargo/数据库/容器/提交窗口。最终静态复核无阻断。默认 Shadow、当前政策、双 epoch、Context、事务原子性、两类暂停、精确输出身份、单次 dispatch、Unknown 和 native owned cleanup 均保持。不扩依赖/feature/平台/模型/硬件范围。

## 最终源码与结果

源码 `d9f97216a6599742c45e0a6389110b6b465e8461`，V4 manifest `bc542c0718b07fc45c1f7e09084a51fddfa3350014eee089514e6a582f9e75ea`，641/641 Git blob 一致。

macOS workspace **1060/0/5**、真实 PTY **3/0/0**、无默认 availability **2/0/0**、提前准入 **1/0/0**；Linux ARM64 Server 默认并行完整测试 **778/0/5**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。

全部最终检查基于相同 V4 输入，阶段前后均无漂移。默认完整测试未设置 RUST_TEST_THREADS；macOS 的 3 个 PTY 用例单独在真实终端运行；另 2 个 ignored 用例需要真实 Easy-Turn 服务，本轮未运行。Linux 未补跑 PTY。两端 CARGO_BUILD_JOBS=2，native Sherpa 使用已有归档，未设置 DOCS_RS。真实 PostgreSQL、loopback HTTP/gRPC、锁与关闭实际执行，模型/外设为原替身。

证据入口：[索引](evidence-index.json)、[macOS 全量](evidence/root/workspace-v4.counts.txt)、[Linux 全量](evidence/linux-arm64-v4/server-tests.counts.txt)、[提交身份](evidence/root/commit-verification.json)、[资源回收](evidence/root/resource-closure.json)。

## 保留的失败与修复链

V1 Linux 因原 registry 只读无法解包本地已有 alsa archive，未运行测试；随后复制现有 registry 到任务目录，全部离线继续。V1 真实 child 16.551 秒，build 16.249 秒。精确实际 sha2 依赖对 462699808 字节保留二进制的相同 64KiB 全文件算法耗时 15.416 秒；digest 与 Python/sha256sum 一致。build 窗口 15 个 PG 样本无活动/等待，不能排除采样间短等待。

仅将 dev profile 的已有 sha2 设为 opt-level=3。同字节优化后 0.834 秒、长度/摘要不变，V2 child 1.280 秒；不改变依赖、算法、输入、OnceCell、release 或原 20 秒期限。最终 V4 默认并行 child 2.983 秒，其中 build 1.833 秒、native release 2ms、schema DROP 87ms。 原 V1/V2 测量及误写旧 binary 路径的命令失败均保留。

macOS V3 全量 **1059/1/5**：setup 取消/无文件断言已通过，最后对旧端口新建连接的断言失败。相同 binary 单测 0.15 秒通过不算修复。端口重用是候选原因，原日志无 peer 身份。V4 仅 cfg(test) 对实际 listener owner 包装，先 drop 原 listener 再发送见证；两个 HTTP 测试等待 serving 返回后立即验证见证，并在原 1 秒上限要求原 accepted socket EOF 或明确 ConnectionReset，其他错误失败。原正常响应、100 Continue、None/无文件、关闭期限均保持；非测试行为不变。V4 targeted **4/0/1** 及完整双平台验收通过。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械及其他平台。L11 历史超时没有阶段日志，不能追认其全部原因；V3 setup 失败当时连接到谁仍未证实。

## 收尾与下一步

任务 PG 为 host 32770 的 `alia_l12_20261010`；未访问用户 5433。停机前其他连接、schema 均为零，全部任务容器已停止；ignored 缓存/原始 binary 留作诊断，没有活动 Cargo 或服务。未推送、未创建 PR，本轮未使用 reset 卡。

下一项软件任务：核对 `packages/alia-server/src/api/skills.rs` 中创建/更新/删除后的三个 reload TODO 与 `modules/interaction/skills` 的实际索引更新路径；先检查既有实现，再补缺口和真实整合回归。保持工具可见性、批准、加载/卸载/热更新和当前政策；本轮未实施该任务。

原 V01、R02 AEC、R03 执行器反馈、Pi/24h/R05、其他平台/R06、前端与未来端上部署仍开放，详见 [延期台账](../../deferred.md)。恢复先读 [当前检查点](../../resume-checkpoint-l12-2026-10-10.md) 与四份 HANDOFF，再检查实际 Git。
