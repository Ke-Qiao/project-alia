# 软件发布资料与真实模型链前置 — 独立审查

2026-10-17；本轮起点 `6a8b1133cf00c4ff6abe9ddc6efc8047029551a5`，应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`。当前 **5/5：只读身份／范围／资料缺口与最终交叉审查完成，GO，无阻断**。本轮资料收口为 10/10，真实模型验收未执行。审查者仅读取 Git、源码、配置示例、文档和既有证据；不探测端点、不运行模型／服务／测试、不连接 Pi 或操作硬件，仅编辑本交接文件。

## Checklist

- [x] 核对当前提交、678 个受测输入及留存 Linux 产物身份。
- [x] 区分当前 Linux、Mac 定向、旧 Mac 全量和历史 Easy-Turn 组件证据。
- [x] 审读现有 operations／配置／模型服务资料，指出确认的资料缺口。
- [x] 整理最小模型与部署确认项，并明确当前不应声明通过的范围。
- [x] 只读交叉核对 Root 汇总后的发布资料／用户确认状态并收口。

## 当前发布来源与可声明结果

本轮起点 HEAD 为文档交接 `6a8b113`，其直接前序为生产修复 `6c62c84`。起点只读重算工作区 678 个输入，与 [最终 source manifest](../2026-10-17-release-lifecycle/evidence/v2/evidence/source-manifest.json) 全匹配，manifest SHA-256 为 `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`。本轮起始已跟踪工作区干净；最终只有下文单项示例注释差异，没有 Rust 行为修改。

同轮 ignored run 下两份独立 Linux ARM64 release 仍存在，重新读取字节后与已验 [inputs](../2026-10-17-release-lifecycle/evidence/v2/evidence/inputs.json) 一致：

| 产物 | 字节数 | SHA-256 |
|---|---:|---|
| `alia-server` | 46,092,208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| `alia-embedded` | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

位置为 `run/validation-20261017-release-lifecycle/v2/artifacts/`。它们是已验 Debian 12 / Linux ARM64 的产物，不是可直接部署到 macOS 的 executable，也不证明目标 Pi Ubuntu 的实际安装／运行已经完成。产物 hash、ELF／动态库及 native 输入通过与部署账户、目录、服务托管和现场资源准备是不同证据。

| 证据 | 源码／结果 | 准确边界 |
|---|---|---|
| 当前 Linux 默认完整 workspace | `6c62c84`；1287 passed / 0 failed / 5 既有 ignored / 0 filtered；67 Running + 5 Doc-tests | 一次原命令，405.963 秒；唯一 Cargo 标题断行以显式派生视图计数，原 INVALID 和原日志保留；不是本轮重跑 |
| 当前 Linux release 与进程 | `6c62c84`；双端 no-default、default release／链接及全 noop 两轮正常启动→SIGTERM→重启通过 | 同配置、账号、身份、shadow 和期望设置恢复；SQLite 只同路径／schema／integrity；未绑定，没有真实模型或物理状态恢复 |
| 当前 Mac 定向回归 | `6c62c84`；sensor owner 两项 GREEN 2/0/0，384 filtered；原实现 RED 1/1 | 确定性取消归属与真实 early-return 对照，不是当前源码 Mac 全量、Mac release 或独立服务验收 |
| 最近 Mac 完整 workspace | `4e2968f`；1277/0/5/0 filtered，677 输入 | [旧邀请历史交接](../2026-10-16-invitation-history/root-HANDOFF.md)；同源 Linux 1285 属该旧版本，不能与当前 Linux 1287 合称当前版本双平台全量 |
| 历史 Easy-Turn 组件 | 2026-09-08；CPU 官方样本 16/16；固定 Torch 2.2.2 的 MPS 实测失败 | [组件验证](../../../../services/easy-turn/VALIDATION.md)；未证明当前 Alia 全链、部署状态、声学／AEC、MPS 成功或当前模型端点可用 |

不将不同版本／不同粒度结果相加，不因新轮只做发布资料而重复已通过的批次。此前 V1 真实 SIGTERM 失败和有界修复已闭环；历史 V01 Companion 配对超时仍 open，不能倒推由该 sensor 缺口解释。

## 起始确认并已由 Root 修正的资料缺口

起始审读 [model-services](../../../operations/model-services.md)、[local-development](../../../operations/local-development.md)、[server-embedded](../../../operations/server-embedded.md)、[hardware-deployment-readiness](../../../operations/hardware-deployment-readiness.md)、[Server 配置](../../../../packages/alia-server/docs/configuration.md)和 Easy-Turn guide 后，确认旧 `model-services.md` 使用已删除的 Embedded `[speech].enabled`。实际 `SpeechConfig` 只有 `channel_id`、`pre_roll_ms`、`upload_queue_frames`，并拒绝未知字段；local-development 已明确不存在此开关。Root 已修正文档为当前模块选择、精确 `speech.channel_id`、绑定／owner 与 VoiceSession 前置，本轮没有实际启用音频。此项是已修正文档缺口，不是待推断的生产故障。

模型服务文档列出 major/minor 和 provider endpoint，但发布记录还应明确 `profiles/llm.toml` 与 `[llm.roles]` 的实际绑定，包括 default、embedding 和可选 memory_capture／sentence_expression。示例默认含 Gemini minor／sentence-expression 和 OpenAI embedding，不能把样例或空 key 视为实际选型，也不能从“major 是本地模型”推断全链无外部请求或付费。device VoiceSession 直接使用 Easy-Turn transcript，不再调用 whisper.cpp；独立 STT 仅在明确选择的 HTTP 转写验收范围内启用。

现有 Easy-Turn 固定栈明确 CPU 为已验路线，auto 选择 CPU，MPS 被已知 guard 拒绝；没有依据在本轮悄然升级 Torch、切换 MPS 或扩为其它平台。服务 README 的安装／约 4.5 GB 下载／启动命令均有副作用，资料核对不等于获准执行；现有本地文件存在也不能替代真实端点／当前运行版本确认。

## 已提问并登记答复的最小项目值

1. **真实模型与首轮范围。** 是否沿用已有固定 Easy-Turn CPU 栈，现有 venv／source／model／Qwen 路径和实际 endpoint；major／minor／default／embedding 及可选角色分别使用的 provider、model ID、API family、服务版本和 endpoint，哪些是本地、哪些可能付费远端。无需发送 key；提供凭据所在受控文件或环境变量名即可。若首轮仅 Easy-Turn 或文本模型链，其余模型保留未验收，不擅自全部启用。
2. **TTS 与素材。** 若本轮包含 TTS，确认 GPT-SoVITS 实际版本／端点、权重和获准使用的参考音频在 **TTS 服务主机**上的路径、语言／参考文本。明确首轮是否仅生成／保存软件结果而不实际播放。Easy-Turn 使用已许可官方／合成素材与当前会话 ID／epoch；真实录音仍留在另行明确的现场范围。
3. **开发运行身份。** 当前开发 Mac 作为 Server 已确认，无需重问主机选择。用户明确允许沿用当前开发账户、任务独立目录，必要时容器隔离，不要求先建立正式服务账户或托管方案；任务配置／key／state／输出路径、数据库与服务归属在后续具体执行包中固定。正式 Mac／Pi 部署、Pi 散热等仍另列现场范围，不阻止本轮资料收口，也不因 SSH 已可用就自动连接或部署。

Root 归并用户问题；本审查者不另行发散询问硬件。R03 用户暂停仍有效，PCA／MG996R 校准和资产实机审核不列为本轮需要用户答复才能完成的资料任务。R02 实物／固件、功放／扬声器、AEC 参考电气／模式／延迟仍照原台账待定。

## 有界出口与当前阻断

软件发布资料可以在本轮完成，并如实附上上述版本和已验产物；**真实模型链的实际执行目前缺项目值确认**，不能凭默认 endpoint、旧日志或配置示例启动服务、下载模型或探测付费接口。没有通过本轮只读审查确认新的生产源码阻断。

将后续验收限定为用户确认的模型链、指定运行方式、固定输入和既有 API／政策，记录实际版本／模型身份、成功与失败分母、初次／后续延迟、原超时／取消和资源退出；保持 Server health 默认 `probe=false`。不因 `/health` ready 就宣布模型效果、自然全双工、AEC 或实际停声通过。当前不声明 Mac／Pi 独立部署、正常绑定恢复、crash／断电、真实录放音、执行器、MPS、长稳或 R05 通过；默认 shadow／权限／执行批准不变，R03 暂停，六轮仍 2/6。

## 发布说明候选补充复核

已只读核对 Root 修订的 `model-services.md`：删除旧 `speech.enabled` 指引，补齐 profile／provider／role、embedding、凭据源、TTS 远端路径与初始化权重切换副作用、配置覆盖顺序、health 与两项 live 测试的边界。未发现新增执行授权或把示例 endpoint 当实际部署值的描述。

Memory 旧说明“启动健康失败后关闭 Memory”确与当前实现不符。`bootstrap/builder.rs` 在 configured Memory enabled 时执行 `verify_memory_health`：Healthy 立即成功；Degraded 记录 keeping memory enabled 并成功；Unhealthy 在 `max(1, health_check_attempts)` 次尝试后返回错误，由原 startup_step 传播导致启动失败，没有静默修改 enabled。Root 改为上述语义准确，不需要为修文档更改生产政策。

另核对 default 角色只复用 minor 或 major、环境变量仅填默认 provider ID 的空 key、TTS `init` 失败切权重仅 warn 后继续、health 接受 GET `/tts` 的 HTTP 400，以及后续 runtime settings 应用晚于 provider 初始化，均与当前实现一致。真实 TTS 验收因此需要受控服务范围和实际 loaded model 身份，不能只凭健康成功宣布指定权重已生效。

本轮 `config/server.example.toml` 仅删除两行旧 STT 纠正角色注释；独立读取受测提交 `6c62c84` 的文件，与当前文件分别用 TOML parser 解析后对象完全相等。没有 Rust 行为修改。**文件字节身份仍发生一项变化**：上文“678 输入全匹配”指本轮起点核对；最终发布资料必须同时保留旧受测清单与这项明确的非 Rust 注释差异，不能把后续当前工作区继续说成 678/678 逐字节相同，也不能将本轮静态解析称为重新运行完整测试。

## 用户答复后的审查口径

Root 已转达本轮用户答复：LLM／embedding 具体模型待定，拟使用 `https://botcf.com/v1`；Easy-Turn／GPT-SoVITS 位于 Developer 目录且有 Juno 权重，Whisper 在项目内，要求参考旧配置；当前是开发验证，不要求独立正式生产账户，必要时可以容器。上述地址和目录说明是用户提供的输入，本审查者未访问该端点或自行扫描相关模型目录。Root 已指定另外两位 agent 做有界只读资料核对。

MP10 已改为“已收到并登记答复”，本轮资料计数收口 10/10；这不表示模型／版本／API family／端点能力或实际加载身份已全部确定，也不表示真实模型验收通过。LLM／embedding 待定项继续如实保留。开发账户和必要容器可沿用用户决定，不能把独立正式生产账户设成当前开发验证的硬门槛；正式 Mac／Pi 服务部署仍另列后续范围。

已交叉核对另外两位 agent 的冻结交接与最终发布资料：旧 base／27 个 fragments 和持久 settings 存在不同方案，不能直接拼成新生效配置；Juno 当前 YAML 指向的两份 GPT／SoVITS 权重仅证明候选文件存在，未选定六份 demo WAV 中的参考音频或匹配 prompt。Whisper 实际 checkout 在 `.local/share/alia/whisper.cpp`，模型文件为 `large-v3-turbo`，与旧说明 `large-v3` 不同；未把用户“项目内”的口述替代只读发现。Easy-Turn 固定 checkout／小 manifest 与三份官方 PCM 摘要的盘点结果已经登记，未将大权重文件的大小、Git HEAD 或 YAML 指向夸大为全部内容冻结、服务 loaded identity 或推理通过。本审查者核对的是上述交接及汇总一致性，没有自行扫描这些模型目录或重做大权重哈希。

最终只读索引 `evidence-index.json` 为 **3 份／8,254 字节**；已独立逐份重算 SHA-256、字节数并与 `run/validation-20261017-model-preflight/` 原件逐字节核对一致。最终记录确认 678 输入中 677 不变、1 项两行注释差异且 TOML 语义相同，两个产物与 148 份既有归档身份保持；276 个本地链接路径检查无失效，范围明确不含 heading anchors。答复状态已为 received，具体模型／embedding 仍显式待定。

最终发布记录、恢复检查点、checklist、Root／README 及另外两份分工交接没有误留资料状态 `9/10` 或“等待答复”；全局台账顶部与本轮资料状态一致。源码、平台与历史失败分母边界准确，没有把 Linux 通过数外推为当前 Mac 完整验收，也没有把本轮资料收口外推为真实模型、硬件、crash／断电、R05 或 V01 根因关闭。独立审查 **5/5，GO**；Root 可按既定范围正常提交。本审查者没有执行服务、容器、测试或网络请求。
