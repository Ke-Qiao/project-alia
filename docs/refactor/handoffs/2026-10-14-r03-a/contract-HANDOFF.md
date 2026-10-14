# R03-A — Core / Server 动作资产契约 HANDOFF

日期：2026-10-14。基线 `2b30dd6`，分支 `codex/architecture-refactor`。Owner：`settings_auth_fix`。本文件随实际验证更新，未勾选项目不表示完成。

## Checklist

- [x] 读取最新恢复检查点、body 设计、Server 账本/签名/表达调用与配置路径。
- [x] 确认旧契约只有 file/fps/loops，没有可信摘要来源。
- [x] 与 Root / Embedded owner 确认用户选择双端本地受控批准清单。
- [x] 添加 Core 与 Server 可运行 RED，Root 已冻结并复现预期失败。
- [x] Core 内容身份、验证辅助与 Body v2 签名契约完成。
- [x] Server 受信配置、首次解析、历史意图恢复及调用方完成。
- [x] 边界回归、独立审查与 Root 统一验证完成。
- [x] 总 checklist、证据归档、恢复检查点和提交由 Root 收口。

当前进度 **8/8**。Core / Server / transport 源码已经停写，独立静态审查无阻断。用户确认的 cue 写入绕过修复完成后，最终冻结版本的 Core / transport / Embedded / Server 统一验证均实际通过；总文档、资源和提交收口由 Root 完成。

## 已确认的信任边界

用户选择 **双端本地受信清单**：操作者审核资产后，在 Server 对应 channel 和 Embedded 本地配置分别固定同一资产版本、SHA-256、字节数。清单缺省为空，不自动信任仓内历史文件。模型只选资产，不提供或改写 pin；本轮不新增资产审核角色、远程清单发布或自动分发协议。

共同类型为 `BodyAnimationAsset { file, version, sha256, bytes }`：file 必须规范 `.anim` 单文件名，version 为大于零的 u32，sha256 为 64 位小写十六进制，bytes 为 1..=65536。`BodyMotion::Animation` 改为包含完整 asset 以及 fps/loops。两端配置名 `approved_animations`，最多 64 项，按规范 file 拒绝重复。Core 复用已有 sha2，提供逐项、清单与同次 bytes 校验；纯摘要计算并不表示审核通过。

Body JSON version 升为 2，请求/响应域分离同步为 `alia-body-v2-*`，旧 name-only 动画不升级补 hash。外层 protobuf 包及服务路径 `alia.body.v1.BodyControl/Exchange` 保留，注释明确封套版本与内容契约的区别。当前控制租约、身份、scope、policy、工具批准、精确取消和 native owner 回收语义保持。

Server 在**首次 body admission**从启动时不可变清单解析，固定 tuple 进入 Entry、持久账本和签名 payload。清单不授予执行权限，原 Response / owner / policy / dispatch permit 检查继续执行。**尚无持久 body 意图时，ExpressionPlan 的文件名未绑定历史资产版本**；本轮不前移到 Response 持久化或改造表达批准体系，不作此类承诺。

已有 cache / durable record 保留原 tuple。清单删改不能阻断对原命令事实的观察，也不能把同一 origin 的内容重绑到新版。Repository lookup 现以原 active owner / response owner JOIN / channel 约束取回事实，并检查 stored owner 一致；调用方从 stored command 提取原 asset tuple，重新解析 raw 后核对完整 command 等值。缓存重复提交还须 raw 完全一致。每个 existing 恢复返回点执行同一核验；claim 的原事务与 same-intent 检查保持。历史事实读取不新增当前执行政策授权，也不创建新 Open / Submit 权限。

Entry 显式保留首次查到的 durable record。即使随后该账本行消失，dispatch 仍只恢复原命令事实，不能把已知 origin 重新当作 fresh 取得发送许可。重启后清单为空或同名资产换版本，原命令的 observe / cancel 继续使用原 tuple；只有新 origin 才能解析当前清单。

## 已识别的影响面

- Core `body.rs`：共享类型、严格 serde、上下界、版本及测试。
- Transport `lib.rs`：两个 body 域分离函数及相关测试；其余鉴权域不变。
- Proto `body.proto`：只更新封套说明，不改变 RPC path。
- Server `config.rs`、`bootstrap/factory.rs`、`channels/embedded/{channel,body}.rs` 与测试：本地静态清单传递及新命令解析。
- Server `modules/body_delivery/repository.rs` 与调用方：保留租户/Response/channel 的读取限制和每个 existing 路径的内容一致性。
- Embedded body capability label 更新为 `embedded_body_v2`，空清单不声明支持 play_animation；必要 fixtures 与 Server example 配置同步，默认动画清单为空且不自动批准任何现有资产。Server 内部 `expression-command-v1` 的动作参数格式未变，保持该内部协议标签。

Server 配置目前没有 channel 清单的 Settings key；新清单也不注册动态设置入口。另发现已有 ChannelAction `update_settings` 可转发任意 JSON 到 Embedded Settings；Embedded owner 已收到并处理本地批准清单不得通过远程 JSON 改写的边界，本分工不修改 Embedded 文件。

## RED 与 GREEN 候选

Core `body::tests::legacy_animation_without_content_identity_is_rejected` 使用旧 JSON `{kind:animation,file:reviewed.anim,fps:30,loops:1}`，原解析与 validate 会成功；新断言要求拒绝。

Server `channels::embedded::body::tests::animation_without_an_operator_approved_catalog_is_rejected` 使用既有 BodyIntent API，普通有效文件名在没有任何批准清单时原实现会接受；新断言要求拒绝。两项均是运行时业务断言，不通过缺失字段编译错误充当 RED。

Root 已真实执行三项 RED，均为预期业务断言失败，exit 101：Core 16.33 秒，Server 55.09 秒，Embedded 同名替换 10.50 秒。输入共 659 项，manifest SHA-256 为 `aff613f253280c38dc44b35f420533153eb04c07f802d00b16d6f39de3a31771`，三份 JSON 均记录 `source_drift: []`。证据位于 `run/validation-20261014-r03-a/red-{core,server,embedded}.{log,json}`，原输入与补丁也保存在该目录。

已写入且已由 Root 在候选冻结版本实际运行通过的 Core / Server / transport 回归包括：

- Core：已知摘要、同长度改字节、长度差异、非法 canonical file/version/hash/bytes、重复/超限清单、旧 name-only 动画拒绝。
- Transport：请求/响应域不可混用、旧 v1 域不能验证 v2、资产内容字段改变后原签名不可验证。
- Server：默认空清单拒绝；模型自报 pin 字段拒绝；一次 canonical file 解析；启动清单 A 换 B 或删除后原记录仍恢复 A、观察/取消不重发；同 origin 的 raw/fps/owner 改变拒绝；已知账本行随后消失不转 fresh；新 origin 才采用 B。
- Server 配置：本地 example 缺省空清单，显式受控 fixture 可加载，重复与无效条目拒绝。
- 真实 PostgreSQL 回归 `animation_ledger_lookup_preserves_owner_response_channel_and_exact_content`：错误 active owner / Response / phase / channel 不取得记录；同 phase 更换 file/version/hash/bytes 的 claim 拒绝，账本仍只有原记录；停用 owner 后读取拒绝。

上述 PostgreSQL 回归由 Root 的独立数据库窗口运行，本 agent 未运行数据库命令。所有 Core/Server/transport 调用方与 fixtures 已迁移并格式化；独立 reviewer 已读回候选，无阻断项。

## 候选统一验证（已读回实际日志）

这批 metadata 的 phase 是 `boundary-red`，构建输入为 660 项，manifest SHA-256 为 `412ba53ed2f412768299a0c47c704f175919f692e6e7de02f96b69609a0d11c0`。不将该版本误写成后续 cue 修复后的最终源码。

| 实际命令范围 | 结果 | 耗时与证据 |
| --- | --- | --- |
| Core + transport `--all-targets --offline --locked` | 41 passed / 0 failed / 0 ignored，exit 0 | 42.11 秒；`candidate-core-transport.{log,json}`；`source_drift=[]` |
| Server 首次 `--all-targets --offline --locked` | exit 101，失败记录保留 | 505.86 秒；`candidate-server.{log,json}`；`source_drift=[]`。Root 确认独立 task DB 在测试前已停止，触发数据库连接超时；该失败不能当作产品断言已通过。 |
| Server 恢复独立 task DB、SELECT 1 后同源重试 | **860 passed / 0 failed / 5 ignored**，exit 0 | 350.07 秒；`candidate-server-retry.{log,json}`；`source_drift=[]` |

Server 计数按 50 个 Cargo `Running` target 的最后结果汇总：lib 369 passed / 3 ignored，integration 491 passed / 2 ignored，main 0 项；排除了 lifecycle 测试启动的嵌套子进程额外一项通过，不能重复计成 861。5 项 ignored 为 3 项真实 PTY 和 2 项真实 Easy-Turn/持续语音服务测试，本轮未执行。真实 PostgreSQL `body_delivery` 8/8、配置 5/5，新 body 内容绑定/重启恢复/账本行消失回归均在本次通过。

再次只读检查完整配置链无遗漏：`ServerConfig::load` 与 `load_candidate` 共用 catalog 校验；factory 将完整清单传到 EmbeddedChannelConfig；channel 创建不可变 Body runtime snapshot；runtime start 再验证直接 typed 构造；首次 admission 固定 tuple，historical 恢复使用原 tuple；所有签名请求/回执复用 v2 域。旧 v1 域只留在降级拒绝测试，内部 expression-command-v1 有意保持。未发现生产构造/lookup 参数漏迁移。

文档检查所发现的现行 `delivery-design.md`“签名 body v1”及 `body-command-design.md`“v1 不提供位置反馈”已由 Root 修正，并已只读核对。R03-A 主段正确说明双端空默认清单、同步升级、首次 admission 绑定以及历史记录恢复；恢复检查点、Root HANDOFF 与 README 的 Server 语义无阻断误述。历史设计/旧 HANDOFF 的当时版本事实不批量改写。

## 最终冻结版本验证

已独立读回 `green-{core-transport,embedded,server}.{log,json}` 及计数；最终构建输入 **661 项**，manifest SHA-256 为 `76a90d0416ae60e8d316a27ff06a0a0457c785659feb8d5235b2d121dfcdf4b0`。三次均 exit 0、`source_drift=[]`，默认 features、默认并行、`--all-targets --offline --locked`，无 filtered 项。平台为 Root 记录的 macOS ARM64；使用 loopback、临时文件、独立测试 PostgreSQL 与 SQLite。

| 最终范围 | 实际结果 | 耗时 |
| --- | --- | --- |
| Core + transport | 41 passed / 0 failed / 0 ignored | 2.42 秒 |
| Embedded（包含 cue 边界修复） | 306 passed / 0 failed / 0 ignored | 98.74 秒 |
| Server（使用最终 Embedded dev-dependency 的组合） | **860 passed / 0 failed / 5 ignored** | 410.01 秒 |
| 合计 | **1207 passed / 0 failed / 5 ignored** | 三次独立运行，不合并为单次时长 |

最终 Server 仍为 50 个 Cargo targets，lib 369 passed / 3 ignored、integration 491 passed / 2 ignored；每个 target 只取最后汇总，排除 nested child 的一项重复结果。5 项 ignored 的真实 PTY/模型条件与候选相同，没有重跑或冒充通过。最终日志保留真实 PostgreSQL、资产配置、历史命令恢复、签名和生命周期回归；之前的 RED、数据库中止及候选记录未被覆盖。

这仅关闭已审软件内容绑定与数据入口写入边界，不能证明机械校准、位置反馈、PWM/力矩保持、真实音频、Linux 应用或 Pi 验收。现有同用户插件部署权能不是 OS 隔离信任域，本包不承诺对抗可执行任意宿主代码的主体。没有新依赖或实际资产自动批准。

剩余 checklist：Root 完成总文档、资源清理、恢复检查点与正常提交后填写最后一项。Core / Server 源继续冻结，此 agent 不运行 Cargo、不提交 Git、不操作实际硬件或额外平台。


## Root最终收口

实现提交`e7265f0d0e9be8d6e663bfcabd53739b32206e27`已正常完成；661个Git blob与最终green manifest逐项一致。Core/transport41、Embedded306、Server860，合计1207/0/5、无filtered、无漂移。本分工最终 **8/8**。本轮PG已移除；共享astrbot后来重启，Root保留它与OrbStack并记录cleanup断言/只读收口。最终总文档与原始证据见本目录README。
