# Project Alia 重构架构提案

**状态：Proposed / 待评审，不代表已实现或完成验收**  
**日期：2026-09-07**  
**目标：保留 Rust 等基础选型，减少通用基础设施的自研，把投入集中在语音交互、个人记忆、人格连续性、主动陪伴和实体表达。**

## 0. 证据与版本边界

本提案依据用户提供的五份架构文档、版本交接说明，以及文末列出的项目官方文档与仓库。没有访问用户电脑上的 Git 工作区，没有核验代码实现，没有重新运行测试，也没有进行真实硬件、音频或性能验收。

按照用户提供的版本说明：`3530395` 是历史基线；其后的已提交更新包含 Web Chat/工作区、早期 Proactive Companion MVP、单所有者认证等；交接中报告的 HEAD 是 `d941ac2`，日期为 2026-07-19；`8c52039` 的早期 Proactive MVP 属于已提交更新。认知与主动表达扩展、后续双渠道投递扩展、声纹实验和可靠性修复等仍有未提交草稿。实际启动重构时必须重新读取 Git 状态，而不能把这份交接中报告的 HEAD 当作已经独立验证的当前版本。

本文中“现有文档说明”只代表文档依据；“建议”“目标”“验收条件”均为新提案。文档未写明某项保证，不等于代码一定没有实现。

## 1. 架构结论

建议采用：

> **Rust 模块化单体 + 独立实时语音运行时 + 分阶段引入的持久化任务执行 + 标准化工具接入 + 能力驱动的多终端表达。**

不建议推倒重写，不建议拆成大量微服务，也不建议把 Alia 改造成另一个通用 Agent 平台。应保留现有三端边界、Rust/Tokio/Axum/Tonic、SeaORM/PostgreSQL/pgvector、Embedded 的 SQLite、Expo/React Native 与现有模型服务适配。

需要重点收紧的不是语言选型，而是：业务状态由谁拥有、一次轮次如何取消、已经发生的副作用如何记录、消息生成与实际投递如何区分、设备断线重启后什么可以重试，以及主动表达依据如何失效。

### 1.1 复用与自研的分界

| 通用部分，优先复用 | Alia 必须保留的产品语义 |
| --- | --- |
| 模型协议适配、MCP 协议、设备生态接入、工作流恢复、向量索引、声学处理、链路追踪、契约生成 | 用户身份与隐私边界、记忆政策、人格和关系连续性、轮次取消语义、主动表达时机、多身体仲裁、工具风险分类、播放与动作的真实完成标准 |

“避免造轮子”不等于把这些业务规则交给一个无法解释内部行为的框架。成熟组件解决通用执行机制，Alia 定义什么叫正确执行。

### 1.2 默认假设与复杂度预算

首先优化单所有者、少量设备、单服务端实例的个人部署。保留未来扩展的标识和边界，但暂不实现 SaaS 多租户、分布式人格集群、完整插件市场或通用工作流编辑器。

第一轮只重构内部边界和可靠性语义，不强制增加新的常驻服务。目标版的后台工作流优先评估并接入 Temporal；只有完成接入验收后才替代对应旧调度路径。不要同时引入 Temporal、Apalis、NATS、Redis 和新的图数据库。

## 2. 现有设计中应保留的部分

现有文档已经明确：前端不参与实体语音热路径；Provider 可替换而核心注册表固定；通道处理自己的打断和播放；记忆拥有规范数据和 `MemoryContextPack`；人格、Affect、Regulation、Expression 分层；动作执行前协商能力并应用安全约束。这些是重构基础，不是需要清除的旧设计。[A1][A3][A4][A5]

优先审查以下交界处：

| 交界处 | 文档依据与风险判断 | 目标 |
| --- | --- | --- |
| Pipeline 与其他模块 | Pipeline 同时承担上下文、路由、工具循环、TTS、投递和持久化，容易积累跨域职责 | 保留薄编排器，数据与状态归属下沉到明确模块 |
| Rules / Scheduler / Insights / Cognition / Initiative | 用户说明中存在多条事件、后台分析与主动行动路径 | 所有用户可见输出和外部副作用进入同一执行与投递边界 |
| 打断与工具 | 语音文档说明被打断后可丢弃工具结果 | 丢弃的是对已取消生成上下文的返回，不是已发生动作的审计结果 |
| 生成与播放 | 文档有流式输出、播放控制和回执 | 生成完成、发送成功、设备播放完成分别建模 |
| 动作 ACK | 文档同时描述 ACK 与多级 action receipts | 审查 ACK 的具体语义；接收确认不能直接冒充物理动作完成 |
| Appraisal 的事实性 | 文档持久化评价事件并重建状态 | 记录“系统做出了某种解释”，而不是把“用户确实如此”视为事实 |

## 3. 目标系统结构

```text
Expo / React Native / Web / CLI
                │ REST / WebSocket / Channel
                ▼
┌──────────────────── alia-server：Rust 模块化单体 ────────────────────┐
│ API / Auth / Settings                                                │
│ Interaction Runtime：会话、轮次、模型调用、流式输出与取消               │
│ Memory：规范记忆、证据、隐私、检索与 ContextPack                       │
│ Companion：认知假设、Affect、Regulation、主动候选与机会判断             │
│ Execution：工具权限、审批、执行账本、外部集成                          │
│ Delivery / Embodiment：规范消息、播放回执、身体仲裁、表达意图           │
│ Adapters：Rig / rmcp / Home Assistant / STT / TTS / PostgreSQL          │
└───────────┬────────────────────┬──────────────────────┬─────────────┘
            │                    │                      │
     gRPC 媒体与控制        PostgreSQL + pgvector    持久任务请求 / Outbox
            │                                           │
            ▼                                           ▼
      alia-embedded                         alia-worker（Rust，后续阶段）
      音频、传感器、动作                                │
      本地安全与 SQLite                          Temporal Service
                                                        │
                                          独立逻辑数据库与迁移管理
```

模型服务依旧作为外部 Provider；不要求它们改为 Rust。复用 C/C++ 声学库、外部模型服务或其他语言编写的开源基础设施，不意味着重写 Alia 的 Rust 主体。

### 3.1 先分模块，不先分进程

| 模块 | 拥有的数据与职责 | 不允许越界 |
| --- | --- | --- |
| `interaction` | Conversation、Turn、上下文组装、LLM 流与取消 | 不直接写记忆表，不直接控制舵机 |
| `memory` | MemoryItem、来源、失效/删除、检索投影 | 不决定开口时机，不直接改变身体状态 |
| `companion` | Observation 引用、认知假设、评价事件、关系投影、主动候选 | 不直接发送消息，不直接执行工具 |
| `execution` | ToolDefinition、Approval、EffectRun、Connector | 不把外部工具返回升级为系统指令 |
| `delivery` | Response、Delivery、播放/动作回执、BodyLease | 不独立生成另一套人格或记忆 |
| `platform` | 用户/设备认证、配置、资源、健康状态、数据库适配 | 不变成所有模块任意读写的巨大服务定位器 |

模块通过明确的应用服务和接口交互。模块只能直接写自己拥有的业务表。需要跨表原子提交的应用用例由统一事务协调，不用“每个模块各提交一次”冒充原子性。

核心业务类型不依赖 SeaORM Entity、Tonic Message 或 Rig Message。框架类型在适配层转换。不要为了抽象而为每个 DTO 新建 crate。

### 3.2 建议目录

```text
packages/
  alia-server/
    src/modules/{interaction,memory,companion,execution,delivery,platform}/
    src/adapters/
    src/bootstrap/
  alia-worker/                 # 第二阶段才引入；复用核心用例
  alia-embedded/
    src/{audio,session,sensors,actuation,device}/
  alia-frontend/
    src/features/
    src/api/generated/
crates/
  alia-core/                   # 共享 ID、纯策略、领域事件、必要接口
  proto/                       # 保留既有 Protobuf 入口
  transport/                   # 保留并限制为传输/认证辅助
  alia-testkit/                # 共享回放、故障模拟、契约测试
```

这是目标逻辑组织，不要求一次性移动所有文件。每次迁移一个已经有特征测试的模块。

## 4. 开源选型与成熟度

### 4.1 推荐采用或保留

| 领域 | 方案 | 接入方式与限制 |
| --- | --- | --- |
| 运行时与 API | 保留 Tokio、Axum、Tonic、SeaORM | 不因重构更换已工作的底座 |
| LLM SDK | Rig，优先仅使用 Provider / completion 能力 | 隔离在适配层；不让其 Agent Runtime 接管 Alia 生命周期；逐个 Provider 迁移并验证 [W1] |
| 外部工具协议 | 官方 Rust MCP SDK `rmcp` | 统一外部工具连接，保留 Alia 的权限、审计与副作用账本 [W2][W3] |
| 智能家居 | Home Assistant REST / WebSocket API | 设备集成与家庭场景交给 HA，Alia 负责语义与权限 [W4][W5] |
| 持久化后台执行 | 目标采用 Temporal + 官方 Rust SDK | 首先试点低风险后台任务，验收后扩展；不用于逐帧音频或逐 token 热路径 [W6][W7] |
| 记忆存储与检索 | 保留 PostgreSQL + pgvector；中文分词可用 jieba-rs | 规范记忆、隐私和关系语义仍归 Alia [W10][W11] |
| 声学处理 | WebRTC AudioProcessing，通过经验证的 Rust 绑定接入 | 不自研 AEC；C/C++ 构建、ARM64、时延和麦扬声器布局必须验证 [W12] |
| 可观测性 | `tracing` 与 OpenTelemetry/OTLP 集成 | SDK 与导出器留在适配层，遥测失败不得阻断对话 [W14] |
| 契约工具 | Buf；openapi-typescript / openapi-fetch | 保留现有 OpenAPI 来源，生成前端类型；加入兼容性检查 [W15][W16] |
| 集成测试 | testcontainers-rs | 用真实 PostgreSQL/pgvector 实例测试迁移与事务，而不只用 mock [W17] |

这些项目并非同等成熟，也不是全部今天立即替换。模型 SDK、音频绑定和新工作流 SDK 必须有独立接入门槛。Rig 当前官方 README 也明确提示后续可能出现破坏性更新，因此应锁定版本、保留自己的 Provider 接口，并先证明它确实减少了维护成本，再替换原有适配器。[W1]

### 4.2 Temporal 的具体决策

本次核对到 Temporal Rust SDK **在 2026-09-04 发布了 1.0.0**，发布正文宣布 Rust SDK 稳定；当前 Cargo 包也标为 1.0.0。部分开发文档仍描述 Public Preview。因此不能再基于旧资料说“Temporal 没有正式 Rust SDK”，也不能把刚发布的 1.0 当作在 Alia 上已经长期验证。[W6][W7][W8]

**建议：把 Temporal 作为目标版唯一的持久工作流方案，采用分阶段替换，而不是第一天整体接管。**

第一个试点建议为“会话结束后的记忆整理/重建索引”。它可延迟、可隔离，副作用可做幂等。随后再验证提醒、长任务、主动候选的等待与过期。交互中的短工具调用继续由 Interaction + Execution 直接管理；跨进程、长时间、需要重启恢复的任务才进入持久工作流。

迁移门槛包括：Rust 工具链和版本兼容、Worker 杀进程恢复、重复 Activity、超时和取消、历史回放、版本升级、Schedule 管理接口所需能力，以及真实部署资源占用。尚未核验 Rust SDK 对 Alia 所需全部调度管理能力的覆盖，不能直接承诺旧 Scheduler 的所有模式都能无损一键迁移。

如果试点不满足要求，维持现有持久调度实现并限制其新增功能，不把整次重构卡在工作流替换上，也不立刻另造第二个框架。

Apalis 可作为轻量任务库备选，但本次官方发布列表的 1.0 路线仍是 `1.0.0-rc.9` 预发布；存在 0.7.4 正式发布，不代表它就是当前长期支持线。因此不把 Apalis 当成“比 Temporal Rust 1.0 更成熟”这个未经证明的结论。[W9]

### 4.3 暂不默认引入

**NATS / Redis / Kafka：** 当前单服务端部署先用进程内消息与 PostgreSQL 持久记录。将来确有多个独立消费者、跨主机事件分发或数据库争用证据，再评估 NATS JetStream。Core NATS 与 JetStream 的持久语义不同；不能把 Core NATS 当作可靠事件仓库。[W18]

**Graphiti / 新图数据库：** Graphiti 提供值得参考的时序知识图谱能力，但当前官方依赖涉及 Python 和图存储后端。Alia 已有规范记忆与轻量图投影，暂不为相似目标增加另一套运行栈。[W19]

**LiveKit：** 有官方 Rust SDK，可在真正需要浏览器/手机远程实时音视频时新增 LiveKit Channel；不要为了未来可能使用，就替换已经工作的局域网 gRPC 链路。其 Rust 实时 SDK 不等于必须采用其他语言的 Agents 框架。[W20]

**Wasmtime：** 面向未来不可信第三方插件的候选隔离运行时，而不是本轮必装依赖。只有真实第三方插件需求出现时再开放受限 host functions。[W21]

**Restate：** 本次核验其服务端许可证为 BSL 1.1；不要把“源码可见”直接记为与 MIT/Apache 相同的无额外限制开源选项。本提案不默认引入。[W22]

## 5. 三类执行路径与资源隔离

### 5.1 实时路径

麦克风采集、AEC/VAD、唤醒、停止播放、播放缓冲、动作限位。这些工作留在 Embedded；任何模型、数据库或后台任务超时都不能阻止本地停止。

CPAL 的回调在专用音频线程上运行；建议回调仅处理预分配缓冲区，不执行网络、数据库、模型推理、阻塞锁或重型分配。推理和序列化放到独立任务或受限工作线程中。[W13]

### 5.2 交互路径

STT、上下文获取、LLM、短工具循环、分段 TTS、投递。每个阶段有时间预算、并发上限、取消传播和降级策略。

不要默认每轮都串行执行一次纠错 LLM、一次路由 LLM、一次情绪 LLM、一次记忆 LLM，再调用主模型。建议以较短路径为默认：能确定的路由用程序规则；记忆检索与独立读取并行；昂贵情绪分析和记忆抽取按需要异步；主生成不等待非必要的完整认知分析。

### 5.3 持久路径

记忆整理、Embedding 重建、提醒等待、主动候选过期、长期任务恢复。它们进入后台 Worker，逐步迁入 Temporal，不占用语音实时资源预算。

建议为前台和后台配置独立的模型并发额度、数据库连接预算和队列上限。仅分文件或分 Tokio task，不能代替资源隔离。

## 6. 生命周期、执行账本与事务

### 6.1 不再只依赖一个 turn_id

建议明确以下标识：

```text
owner_id / actor_id / identity_assurance
conversation_id
turn_id + generation_epoch
response_id
channel_id / endpoint_id / device_boot_id
playback_id / command_id / effect_id
event_id / correlation_id / causation_id
schema_version / policy_version / config_revision
```

并非每条消息都携带全部字段；由场景选取必要字段。`channel_id` 表示入口或适配器实例，不应同时承担“用户身份”“物理身体”“正在播放的回答”等多种职责。

### 6.2 四类状态各自独立

```text
Turn:      accepted → running → completed | cancelled | failed
Response:  drafting → committed | interrupted | discarded
Delivery:  pending → accepted → started → completed
                       └→ failed | cancelled | unknown
Effect:    proposed → approved → dispatched → succeeded
                                   └→ failed | cancelled | unknown
```

这些状态图是目标语义示意；具体状态转移必须按现有代码设计兼容迁移，不能直接批量替换枚举。

**消息生成完成不等于投递完成；投递完成不等于用户听到；命令被接收不等于物理机构到位。**

### 6.3 打断不能抹掉现实动作

现有语音文档允许工具继续执行，并在轮次取消后丢弃结果。[A2]

建议精确定义：

- 对已取消的 LLM 上下文，不再继续喂工具结果，也不继续合成回答。
- 对已提交或已经执行的外部副作用，必须保留独立 EffectRun 的最终状态或 unknown 状态。
- 下次对话需要了解“刚才是否已经关灯”时，查询执行事实，而不是依赖一条不存在的完整助手回复。
- 对可安全取消的只读任务可以取消；对已经发出的不可撤销动作不宣称自动撤销。需要补偿时作为新的、经过授权的动作。

Tokio CancellationToken 可复用作进程内协作取消机制，但它不是外部服务或硬件的撤销证明。[W23]

### 6.4 Outbox 解决业务提交与执行触发的间隙

建议在同一个 PostgreSQL 事务中提交：业务记录、审计/状态变化，以及对应的 Outbox 意图。异步投递器再把意图送给 Worker、Temporal 或设备控制路径。

例如：提交提醒记录与启动工作流的请求必须一起落库；若数据库已提交而进程在启动 Workflow 前崩溃，Outbox 重试仍能恢复启动。Workflow ID 与业务请求 ID 建立稳定映射，重复启动不能产生两个业务提醒。

Outbox 不需要发展为自制消息中间件。它只处理 Alia 业务事务与外部执行机制之间的原子性缺口。PostgreSQL NOTIFY 最多用作唤醒提示；可靠性必须来自可重新扫描的持久表，而不是假定所有通知都被消费。[W24]

### 6.5 不承诺物理世界的 exactly-once

数据库与工作流系统不能把任意硬件或 HTTP 副作用自动变成“恰好一次”。例如设备动作已执行，但在记录回执前断电，恢复后无法仅凭数据库判断是否执行过。

建议每种 Tool / Action 声明：风险等级、是否幂等、幂等键、超时、重试策略、是否可取消、是否支持状态核对、是否存在补偿动作。

“设置灯为关闭”与“切换灯状态”的重试语义不同。重复的传输尝试复用 effect_id；用户后来再次发出的新指令使用新 effect_id。

对于不可幂等且无法核对结果的动作，超时后进入 unknown，禁止无条件重试。Temporal 的 Activity 也必须按可能重复执行来设计并进行故障测试。[W25]

## 7. 语音与 Embedded 重构

### 7.1 保留 gRPC，但区分三种“流式”

现有文档写的是捕获 utterance 后上传，并在上传、LLM、TTS、回放多处存在流式接口。[A2]

需要分别声明能力：

1. 采集过程中是否持续上传；
2. STT 是否真正支持增量转写及 final transcript；
3. TTS 首个可播放音频是否能在全文完成前生成并被播放。

不能因为网络采用 streaming RPC，就宣称已经实现端到端流式或自然全双工。没有稳定 AEC、播放参考流和双讲测试时，应准确标为“支持唤醒词打断的连续对话”。

### 7.2 端侧音频链路

```text
麦克风 → 音频缓冲 → AEC/降噪 → 唤醒/VAD → 会话采集 → gRPC
                     ↑
              实际扬声器播放参考流

gRPC → 有界播放队列 → 播放器 → 扬声器
                  └→ 播放回执与本地立即停止
```

WebRTC AudioProcessing 可处理回声、噪声等，建议复用；但 Rust 绑定构建、音频格式、渲染/采集时序和真实硬件必须单独验证。[W12]

媒体与控制至少使用分离的队列、独立的背压与优先级策略。停止播放不能排在一长串音频块后面；必要时使用独立连接，防止共享流控造成控制延迟。

输入音频也不能无限排队。过载时要有明确的丢弃、拒绝或重新采集策略；损坏的 utterance 不应悄悄进入可能执行设备动作的识别链路。

### 7.3 防止旧回答“复活”

每个音频片段关联 `turn_id + generation_epoch + playback_id`。Embedded 接受新轮次或执行打断时，使旧 generation 失效；即使旧音频晚到，也不能进入播放队列。

设备重启后切换 `device_boot_id`，失效旧的播放计划、会话窗口与未核对动作。用户在重启前可能听到一半的回答，不在重连后自动重播。

### 7.4 播放与动作的证据等级

播放回执区分 received/accepted、started、device_completed、cancelled、failed、unknown。即使播放器报告完成，也只能证明设备端播放流程完成，不能证明用户实际听到。

动作回执区分“接收命令”“开始执行”“规划轨迹结束”“传感器确认到位”。没有位置反馈的舵机不能伪造实际到位测量。前端和审计都展示证据等级。

### 7.5 本地安全与离线状态

建议 Embedded 独立维护硬静音、音量上限、夜间策略、动作限位、速度限制、超时和心跳策略。机构断连后的安全状态必须针对具体结构验证，不能一概假定断电就是安全。

声学处理、ONNX、OpenCV 等可能包含 native 依赖。需要真正崩溃隔离时，可把易失败的感知模块放在独立进程；不能把“模块返回 Result”当成进程崩溃隔离。

服务器断线时只开放明确白名单的本地提示和已授权离线行为。不缓存一大串语音或动作，等待联网后无条件补发。

## 8. 记忆、认知和主动陪伴

### 8.1 保留 Alia 的规范记忆

现有 `memory_items`、Embedding profile、轻量图与 `MemoryContextPack` 应保留；它们已经定义了用户可编辑、可删除、可暂停的产品边界。[A4]

底层检索继续使用 pgvector 与 PostgreSQL 全文检索，必要时通过 RRF 合并排序；中文内容可在写入和查询时采用相同版本的 jieba-rs 分词规则。不要把英文示例直接当作中文召回质量已合格的证据。[W10][W11]

新增或强化：来源消息/事件引用、有效起止时间、矛盾与 superseded 关系、用户确认状态、隐私范围、版本、删除/遗忘标识。

Embedding 是派生索引，不是规范记忆。替换模型时登记新的 embedding space，异步重建并验证后切换；不同模型即使维度相同也不能默认直接混查或比较分数。

### 8.2 删除必须阻止后台复活

删除或暂停记忆，不仅要影响 API 列表，还要影响检索投影、关系摘要、缓存、在途提取任务和待投递主动内容。

建议任务携带用户/记忆版本或 privacy_epoch，提交时再次核对。用户删除后，使用旧版本提取结果的 Worker 不得重新写回。恢复备份时也需应用删除记录与隐私策略。

审计记录保留最小必要元数据，不通过“审计需要”永久保存用户已经要求清除的原始私人内容。确需保留的记录与备份保留期限必须在产品中明确。

### 8.3 认知闭环的目标边界

```text
Observation（设备报告/用户表达/系统事件）
  → Interpretation（可出错的解释与假设）
  → AppraisalEvent（有证据、置信度、有效期的评价）
  → Affect / Relationship / Resource 的派生状态
  → Regulation
  → InitiativeCandidate（为何值得做什么）
  → 独立 Opportunity（现在是否有交互条件）
  → Policy Gate（是否允许做）
  → ExpressionIntent / ToolProposal
  → 正常生成、执行与 Delivery
  → 回执和用户反馈
```

保留 Persona → Affect → Regulation → Expression 的已有分层，不让 LLM 直接写关系分数、调用舵机或绕过投递策略。[A5]

`AppraisalEvent` 的持久化表示“某版本解释器做过这个判断”，而不是“判断一定为真”。应保存来源引用、解释器版本、有效期和反馈，以便重新评估。

Reducers 接收显式时间与版本，尽量为纯函数；重放时不能重新调用实时 LLM 或实时传感器，并假装重放的是当时状态。外部模型结果需作为有版本的输入记录。

### 8.4 主动表达的具体规则

程序先决定是否值得考虑，再调用 LLM 形成表达；不要每隔几秒运行一次大模型“自由思考”。内部连续性可以由事件、待处理意图、受限动机与状态衰减构成，不需要无限增长的内心独白文本。

主动候选至少包含：来源证据、用户/主题、行动理由、有效期、重复抑制键、建议渠道、必要身份保证与隐私等级。

机会判断至少检查：用户允许主动联系、数据新鲜度、当前是否忙碌/静音/睡眠、是否已有主动或被动对话、是否有可用输出终端、同主题是否已经说过、每日/窗口预算，以及用户是否对上次表达作出回应。

候选生成之后、真正投递之前再次核对权限、证据有效期和时机，防止“生成时允许、播放时已经不允许”。

“有人存在”不等于“用户本人在场”；“没有回复”不等于“不喜欢 Alia”；模型推断的负面情绪不能直接写成用户事实，也不能成为施压、催促或降低服务质量的依据。

用户暂停主动关怀、要求安静或修改隐私设置，应直接更新硬约束，不应通过某个关系分数再决定是否服从。

### 8.5 草稿的接入路径

先做离线回放，再做不发送消息、不执行硬件的 shadow mode，最后小范围启用真实投递。人格实验室的模拟机会仍是模拟机会，不作为真实感知和现场体验验收。

声纹继续作为外部独立项目，不设为本轮依赖。预留 `IdentityResolver` 和结果结构即可；不能把预留接口描述为已实现身份识别。

## 9. Web、Voice 与身体仲裁

### 9.1 一份规范内容，多份投递状态

对于同时提供 Web 留档和 Voice 输出的主动回答，先形成经过策略检查的规范 Response。Voice 复用同一版本的内容，而不是再次生成一份可能含义不同的话。

交互式流式回答可以按已提交片段冻结，再合成为最终记录；不能为了“一份完整冻结文本”强迫所有普通对话等待全文完成。

建议持久化结构：

```text
responses：规范内容、版本、内容 hash、生成状态
response_segments：需要流式时的已提交片段
response_deliveries：每个 channel 的投递状态
playback_receipts：设备播放证据
effect_runs / action_receipts：外部动作与硬件证据
```

被打断的内容可以保留为受保留策略管理的 interrupted 记录，但不能在上下文中冒充完整送达的回答。Web 显示了完整内容与 Voice 只播放了一部分可以同时为真，必须分别记录。

### 9.2 身体资源租约

建议引入有限作用的 `BodyLease`，由 Delivery/Embodiment 统一管理主动输出身体与资源优先级。它不是分布式集群锁平台，而是个人部署中的输出仲裁状态。

默认同一人格可以接收多个入口的观察，但同一时刻只有一个主动语音输出位置；Web 可保留文本历史而不重复开口。物理与 Web 的激活/休息状态遵循用户选择和可用性，不因页面重连就改变人格状态。

租约包含 epoch、期限、endpoint 和能力版本。设备执行计划前验证租约与最新能力；断线或租约过期后停止主动输出，禁止旧进程继续驱动身体。

## 10. 工具、插件与智能家居

### 10.1 所有工具经过同一执行入口

```text
LLM / Rule / Workflow / Initiative
              ↓
       ToolProposal + typed parameters
              ↓
Identity / Capability / Risk / Approval / Budget
              ↓
          EffectRun ledger
              ↓
Built-in executor / MCP / Home Assistant / Device adapter
              ↓
       Verified receipt / unknown
```

模型原生 tool calling 是请求表达方式，不是执行授权。工具定义应由程序登记，LLM 只在当前允许的候选范围内提出调用。

### 10.2 MCP 的边界

使用官方 `rmcp` 实现协议、连接和工具交互，不重复造同等协议层。[W2]

工具描述、返回内容、网页、技能资源和记忆文本一律按其来源处理，不把外部内容当成系统级指令。MCP 不自动替 Alia 完成业务授权；官方安全文档也要求最小权限、令牌边界与本地进程安全。[W3]

对本地 stdio MCP 服务，安装/启动意味着执行本地程序；必须有明确安装许可、版本固定、权限清单和隔离策略，不能把任意 manifest 中的命令直接当可信配置执行。

### 10.3 Home Assistant 的边界

HA 负责其设备生态中的实体、状态、服务动作和家庭自动化；Alia 通过 REST / WebSocket 获取状态和调用已授权动作。[W4][W5]

同一条自动化只能有一个规范定义和执行归属。纯设备联动优先放 HA；需要个人记忆、语境、表达或用户确认的逻辑由 Alia 管理。不要两边各存一份稍有差异的规则。

### 10.4 三种扩展，不做三个万能插件框架

- 随 Alia 发布的可信核心扩展：Rust trait / 编译期注册。
- 外部工具和服务：MCP 或正式服务 API。
- 将来不可信的轻计算扩展：评估 Wasmtime；只开放最小 host capabilities。

既有 JSON stdin/stdout 表达插件可以作为兼容路径保留，但清空环境变量不是完整沙箱。文件、网络、进程、密钥和 CPU/内存权限必须另行限制。Wasmtime 自身的沙箱也不能弥补宿主函数开放了过大权限的问题。[A5][W21]

## 11. 身份、配置、前端与运维

### 11.1 身份与安全

保留现有单所有者方向，不在重构中无故恢复复杂多用户系统。区分“设备已绑定”“请求来自授权终端”“当前说话者被可靠识别”。设备可信不自动等于说话者是主人。

未验证身份默认只开放非私人、低风险交互；受信任家庭模式必须是显式配置且有可见限制。声纹结果未来作为证据输入，而不是唯一的高风险授权因素。

保留成熟密码哈希和令牌库，补齐令牌轮换/撤销、接口权限、密钥最小暴露与传输加密。设备通信建议基于 TLS，绑定后再考虑双向认证。Ed25519 签名不能替代传输加密；mDNS/BLE 发现不是信任证明。

高风险执行、私密记忆检索和认证配置加载采用 fail-closed。某模块异常时“尽量继续服务”只适用于有安全降级路径的能力。

### 11.2 配置只有明确的一组有效值

保留 TOML 启动配置与片段合并。启动专用参数不能被运行时 API 随意修改；可热更新项由注册表定义。

区分 `desired_value`、`effective_value`、`revision`、`applied_revision` 与应用结果。运行时 override 优先级明确；恢复默认是移除 override，不是改写仓库 TOML。热应用失败时保留旧有效值，返回可追踪失败，而不是前端显示成功但运行时未变。

资源和密钥使用引用，不把密钥内容嵌入一般配置导出、日志、MCP 请求或模型上下文。

### 11.3 前端不重写

保留 Expo、React Native、Expo Router、SWR、Uniwind。重构重点是生成 API 类型、统一领域状态、支持失败/unknown/degraded 状态以及设备能力驱动的界面。

REST 提供规范快照，WebSocket 提供增量事件；断线重连使用版本/游标检测缺口，必要时重新拉取快照。前端不能靠猜测事件一定完整而长期保持错误状态。

前端展示“已发送”“设备开始播放”“设备报告播放完成”“结果未知”，不要统一显示“成功”。配置 UI 同时显示期望值与实际生效值。

### 11.4 部署与升级

首轮仍运行现有 Server + PostgreSQL + Embedded + Frontend。第二阶段启用 Temporal 时，后台 Worker 独立部署，核心语音不依赖工作流服务可用性。

Temporal 可以使用 PostgreSQL 作为持久化与 Visibility 存储，不需要为此默认增加 Elasticsearch。但 Alia 业务数据库和 Temporal 数据库应有独立权限与迁移工具，Alia 不直接访问或修改工作流引擎内部表。[W26]

开发态可用 Compose；正式部署不能把临时开发服务器当作长期运行保障。内部工作流服务不暴露公网。[W27]

模型、音色、动画、插件包登记版本、校验和和适用能力。Server/Embedded 协议采用可协商版本和 N/N-1 兼容测试，删除字段保留编号，升级遵循先扩展、后迁移、最后移除旧字段。[W15]

数据库与资源均需要备份和恢复演练。二进制回滚与数据库回滚是不同问题；破坏性迁移不能仅靠切回旧程序解决。

## 12. 可观测性与验收

### 12.1 可观测性

建议统一 trace/correlation 标识，贯穿唤醒、音频上传、STT、模型路由、检索、工具、TTS、播放和动作回执。保留业务执行账本与隐私审计；不要把所有业务记录混进高容量调试日志。

复用 OpenTelemetry 标准和导出能力，而不自研追踪后端。本次官方 Rust 页面仍把 traces/metrics/logs 标为 Beta，所以相关 SDK 也应锁版本并在适配层隔离。[W14]

默认不把原始语音、完整私人记忆、密钥或全部提示词发送到遥测后端。采样的运行追踪与必要业务审计分开设置保留期限。

### 12.2 建议验收目标，不是当前实测结果

| 场景 | 建议目标与测量边界 |
| --- | --- |
| 本地打断 | 从本地确认打断事件到停止声学输出，p95 ≤ 100 ms；不包含唤醒词模型识别时间 |
| 唤醒反馈 | 从确认唤醒到本地提示开始，p95 ≤ 150 ms；资源缺失不能阻断主流程 |
| 首音 | 固定硬件、暖模型、局域网、无工具调用条件下，用户语音结束到第一段有效回答音频 p95 ≤ 3 s；单独记录端点检测预算 |
| 取消安全 | 连续打断、乱序、重连、设备重启后，旧 generation 不得恢复播放 |
| 工具可靠性 | 重复请求不产生非预期重复副作用；无法判定执行结果时进入 unknown |
| 主动关怀 | 静音、权限撤回、证据过期、无机会时不投递；不确定播放结果不自动重播 |
| 记忆隐私 | 删除/暂停立即影响在线检索；旧后台任务不能重新写回被删除内容 |
| 长时运行 | 在规定硬件上进行至少 24 小时混合负载测试，检查内存、句柄、音频欠载、队列与温度 |

这些数字是首轮协商用目标，必须用当前设备和模型建立基线，再决定最终发布阈值。不能把某个模型 API 的平均响应时间作为整条语音链路的验收结果。

### 12.3 必测故障

必须覆盖：生成中打断、播放中打断、工具已执行但回执丢失、Worker 执行后崩溃、数据库提交后服务崩溃、重复/乱序消息、设备重启、权限在投递前撤回、记忆删除时抽取任务仍在运行、能力快照过期、麦克风与扬声器回声、配置热应用失败。

使用 testcontainers-rs 做数据库和迁移集成测试；用确定性时钟与脱敏事件集做认知回放；用模拟设备做协议故障测试；最后进行真实麦扬声器与机构的 hardware-in-the-loop 验收。[W17]

## 13. 迁移路线与交付门槛

| 阶段 | 范围 | 可交付与验收 |
| --- | --- | --- |
| P0：建立可追溯基线 | 读取实际 HEAD、已提交历史、暂存与未暂存变更；保存安全快照 | 区分 3530395、其后已提交能力和四类草稿；不执行破坏性重置，不把密钥打包进快照 |
| P1：行为与契约先行 | 轮次、播放、工具标识；状态机；执行账本；回执语义；关键 trace | 现有语音回归通过；旧音频不复活；已执行工具有独立记录 |
| P2：模块抽离 | Interaction / Memory / Companion / Execution / Delivery 的写入归属 | 无跨模块任意写表；现有功能与 API 兼容；本阶段不顺带大改前端 |
| P3：通用能力替换 | Rig Provider 小范围迁移；rmcp 只读工具；Temporal 低风险后台试点 | 逐项契约和故障测试；每项有旧路径回退开关；不能同时切所有基础依赖 |
| P4：主动闭环 | 离线回放 → 无副作用 shadow → 受限真实投递 | 证据、机会、去重、静音、撤权、内容冻结和回执共同验收 |
| P5：发布收敛 | 兼容窗口、升级、恢复、资源版本、真实硬件 | 长时运行和用户体验验收；通过自动测试不等于产品完成 |

每个改动包都需要：范围说明、行为不变量、迁移说明、测试证据、可观测字段、回滚方式，以及“明确不在本次范围”的列表。Agent 可以执行任务，但不能以“代码已生成/测试已通过”替代硬件或体验判断。

### 首批建议 ADR

1. 模块化单体与数据写入归属。
2. Turn / Response / Delivery / Effect 四类生命周期。
3. 同事务 Outbox、幂等键和 unknown 结果处理。
4. Media 与 Control 的背压、取消及协议版本。
5. Memory 规范数据、派生索引与删除传播。
6. Cognition 假设、独立 Opportunity 与主动输出 Gate。
7. BodyLease 与 Web/Voice 内容一致性。
8. Temporal 试点范围、升级与退出标准。
9. MCP/插件权限模型与 HA 自动化归属。

## 14. 最终建议

第一优先级不是再增加智能能力，而是让已经存在的能力在失败、打断、重连、重启与权限变化时仍保持可解释的一致行为。

保留 Alia 的记忆政策、认知与人格闭环、主动时机和身体表达；将模型协议、工具生态、智能家居接入、持久执行、声学处理、观测与契约工具交给成熟组件。这样未来更换模型、接入新的身体或扩展渠道时，变化主要发生在适配层，而不需要重新改写人格和交互核心。

---

## 资料依据

### 用户提供的架构材料

- [A1] `overview(1).md`，重点为 Runtime Responsibilities / Design Direction，原文约 L37–L65。
- [A2] `voice-pipeline(1).md`，重点为 End-to-End Turn、Interruption and Barge-In、Streaming and Latency，约 L12–L30、L71–L96、L117–L135。
- [A3] `architecture(1).md`，重点为 Main Layers、Provider Selection And Core Services、Failure Model，约 L7–L47、L77–L91。
- [A4] `memory(1).md`，重点为 Storage、Capture Modes、Retrieval、Consolidation、Runtime APIs，约 L18–L158。
- [A5] `expression-runtime(1).md`，重点为 v0.5 Scope、Persistence、Extension Points、Channel Actions，约 L6–L64、L212–L252、L290–L326、L422–L454。

以上行号对应本次材料呈现范围，用于导航；以文件实际章节内容为准。用户在消息中的历史版本与草稿状态说明另行作为版本边界依据。

### 外部官方资料（核验日：2026-09-07）

- [W1] Rig 官方仓库；Provider 与 Agent Runtime 边界：`https://github.com/0xPlaygrounds/rig`
- [W2] MCP 官方 Rust SDK：`https://github.com/modelcontextprotocol/rust-sdk`
- [W3] MCP 安全最佳实践：`https://modelcontextprotocol.io/docs/2026-07-28/tutorials/security/security_best_practices`
- [W4] Home Assistant WebSocket API：`https://developers.home-assistant.io/docs/api/websocket/`
- [W5] Home Assistant REST API：`https://developers.home-assistant.io/docs/api/rest/`
- [W6] Temporal Rust SDK 1.0.0 发布：`https://github.com/temporalio/sdk-rust/releases/tag/v1.0.0`
- [W7] Temporal Rust SDK Cargo 元数据：`https://raw.githubusercontent.com/temporalio/sdk-rust/main/crates/sdk/Cargo.toml`
- [W8] Temporal Rust Worker 文档；核验时仍有 0.7/Public Preview 表述：`https://docs.temporal.io/develop/rust/workers/worker-process`
- [W9] Apalis 官方发布列表：`https://github.com/apalis-dev/apalis/releases`
- [W10] pgvector 官方仓库，Hybrid Search：`https://github.com/pgvector/pgvector`
- [W11] jieba-rs 官方仓库：`https://github.com/messense/jieba-rs`
- [W12] webrtc-audio-processing Rust 绑定说明：`https://docs.rs/crate/webrtc-audio-processing/latest`
- [W13] CPAL 官方 crate 文档：`https://docs.rs/cpal`
- [W14] OpenTelemetry Rust 官方状态说明：`https://opentelemetry.io/docs/languages/rust/`
- [W15] Buf breaking change detection：`https://buf.build/docs/breaking/`
- [W16] openapi-typescript 官方仓库：`https://github.com/openapi-ts/openapi-typescript`
- [W17] testcontainers-rs 官方仓库：`https://github.com/testcontainers/testcontainers-rs`
- [W18] NATS JetStream 官方说明：`https://docs.nats.io/concepts/jetstream`
- [W19] Graphiti 官方仓库：`https://github.com/getzep/graphiti`
- [W20] LiveKit 官方 Rust SDK：`https://github.com/livekit/rust-sdks`
- [W21] Wasmtime Security：`https://docs.wasmtime.dev/security.html`
- [W22] Restate 服务端许可证：`https://github.com/restatedev/restate/blob/main/LICENSE`
- [W23] Tokio Graceful Shutdown：`https://tokio.rs/tokio/topics/shutdown`
- [W24] PostgreSQL NOTIFY：`https://www.postgresql.org/docs/current/sql-notify.html`
- [W25] Temporal 生产前故障测试：`https://docs.temporal.io/best-practices/pre-production-testing`
- [W26] Temporal PostgreSQL Visibility：`https://docs.temporal.io/self-hosted-guide/visibility/postgresql`
- [W27] Temporal 自托管部署：`https://docs.temporal.io/self-hosted-guide/deployment`

说明：软件仓库的 main 分支与 latest 文档会变化；正式实施必须记录具体 release/tag、Cargo.lock、镜像 digest、许可证与接入测试结果。本提案不是已锁定版本的依赖清单。
