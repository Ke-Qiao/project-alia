# 软件发布资料与真实模型链前置

更新：2026-10-17。资料基线 `6a8b113`，当前已验应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`，分支 `codex/architecture-refactor`。本页汇总已有软件证据和后续模型验收所需输入；不是已完成真实模型部署或硬件发布的声明。本轮只读核对、修正文档与示例注释，没有探测端点、启用服务、运行模型或安装依赖。

## 当前可确认的软件出口

| 平台／层次 | 受测版本与结果 | 结论边界 |
| --- | --- | --- |
| Debian 12 Linux ARM64，Rust 1.95 | `6c62c84`：默认workspace 1287 passed / 0 failed / 5既有ignored / 0filtered；两端no-default all-targets、default release与native链接通过 | 当前Linux构建／测试／链接出口；不是Mac或Pi部署结果 |
| Linux全noop双端进程 | 同版本两轮启动→SIGTERM正常关闭→同配置／DB／身份／期望设置恢复；四进程exit0，无强杀，端口关闭、任务DB连接归零 | 未绑定的软件实例；不是绑定恢复、crash／断电或真实设备／模型验收 |
| 当前Mac | `6c62c84`两项取消归属回归通过，原RED保留 | 当前源码的定向证据，不是当前Mac完整release／整链出口 |
| 最近Mac完整workspace | 旧源码`4e2968f`：1277/0/5 | 仅历史；不能与当前Linux结果合称同源双平台全量 |
| 真实Easy-Turn | 2026-09-08固定CPU组件16/16；09-28两项live目标历史通过 | 历史模型组件／Easy-Turn软件集成，不认证当前部署或完整LLM/TTS链 |

当前5项ignored仍为3项PTY、2项Easy-Turn；本轮没有改变忽略状态或追加动态测试。原测试期限、默认并发和资源owner规则保持。详细原件见[上轮四份HANDOFF](../refactor/handoffs/2026-10-17-release-lifecycle/README.md)、[148份索引](../refactor/handoffs/2026-10-17-release-lifecycle/evidence-index.json)及[模型验收边界](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。上轮一处Cargo标题分块有独立派生行映射，原日志、原INVALID计数和失败结果均保留，不是重跑求绿。

## 发布产物、配置与恢复资料

以下为已经保留并重新只读核对的Linux产物，不能在Mac上直接作为本机可执行文件使用。实际Mac发布还需对应平台的构建、链接和运行证据；此处不扩展到其他平台。

| 文件 | bytes | SHA-256 |
| --- | ---: | --- |
| `alia-server` | 46092208 | `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616` |
| `alia-embedded` | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

独立副本位置：仓库ignored目录 `run/validation-20261017-release-lifecycle/v2/artifacts/`。对应受测678输入manifest为 `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`，逐Git blob匹配`6c62c84`；实际copy／链接审计／两轮进程摘要一致。本轮资料修订只删除Server示例中两行过期注释，TOML语义前后相同；不把注释修订说成一次新构建或篡改旧manifest。

- 配置结构与敏感值来源：[Server配置](../../packages/alia-server/docs/configuration.md)、[模型服务契约](model-services.md#configuration-and-acceptance-boundaries)。base、排序`.d`、env补空规则、persisted settings及SQL政策应分别核对，不能只看一份TOML。
- 无设备准备：[Embedded staging模板](../../config/embedded.staging.example.toml)，仅用于私有目录的全noop实例。通用[Embedded示例](../../config/embedded.example.toml)选择真实采播，不能当成无设备预设。
- 启停与正常恢复：[无前端操作入口](server-embedded.md#配置与启动)。首次设备ID写回配置是预期，保留key、state、配置及Server settings；设置已保存不保证模块应用成功。SQLite完整性与schema通过不等于文件字节或物理状态相同。
- 失败发布与重试：[配置发布恢复](../refactor/config-durability-2026-10-11.md)、[运行时设置部分成功](../refactor/settings-recovery-2026-10-11.md)。保留原阶段错误和持久身份，不以删除恢复记录或覆盖SQL政策代替核对。
- 旧任务容器／卷／network已回收，共享服务保留；ignored私有目录不是可直接复用的完整部署。新验收先确认独立目录、数据库及资源归属，不能凭旧容器名继续启动。

## 模型与部署输入登记

已确认目标仍为当前开发Mac运行Server，Easy-Turn位于Server端；Pi5不运行Easy-Turn，R03继续暂停。用户已答复三组问题：对话与embedding具体方案待定，拟使用`https://botcf.com/v1`测试；Easy-Turn与GPT-SoVITS在Developer目录，TTS使用Juno权重，Whisper在项目内，按旧配置核对；当前是开发验证，必要时可用容器。开发验证沿用当前账户与任务临时独立目录，不要求先建立正式服务账户／托管方案；正式Mac/Pi部署另行验收。

| 输入 | 已有契约／需登记字段 | 当前实际值 |
| --- | --- | --- |
| major／minor聊天 | provider ID、base URL、API family、模型ID及版本、认证来源；default选major或minor | 拟测试基址`https://botcf.com/v1`；精确模型／API能力／认证来源仍待定，未访问该地址 |
| 可选聊天角色 | memory_capture专用或minor fallback；sentence_expression专用或确定性规则；逐项核对是否远端调用 | 待用户确认，示例不代表选择 |
| embedding | OpenAI兼容端点、独立模型ID／版本、实际维度、认证来源及SQL active profile | 用户明确待定；不假定拟用基址支持embedding，不擅自关闭Memory/cache或重写政策 |
| Easy-Turn | 既有隔离环境、source/checkpoint/Qwen目录及修订，loopback端点；仓库固定Python3.10 CPU栈 | 用户指定本地Developer目录，目录盘点见下方交接；运行状态与实际端点未验 |
| GPT-SoVITS | 服务版本、完整`/tts`地址、voice、服务端参考音频/文本/语言与GPT/SoVITS权重身份；独立还是共享实例 | 用户指定Developer内服务及Juno权重；精确组合／参考素材／loaded identity仍需核对 |
| 独立Whisper转写 | 是否列入首轮显式HTTP转写范围；基址、服务版本、实际加载模型、样本语言 | 用户指定项目内资产；首轮范围未定，VoiceSession不需要第二次STT |
| Mac开发验证 | 当前账户，任务独立配置／key／state／输出路径，PG及服务归属 | 可沿用开发账户、必要时容器隔离；本轮不创建容器，正式服务账户不作当前门槛 |
| 认证及输入输出 | 受控文件位置或变量名；可发送的非敏感文本／离线PCM，私有输出目录，若远端计费则有限调用范围 | 待确认，不收集密钥值，不自动访问外部端点 |

只给模型ID或一个URL不足以证明加载了指定权重；记录服务版本／部署清单与实际加载身份。完整GET settings可能包含敏感原值，不应直接归档；TTS参考路径按TTS主机解释。依赖、资产缺失应标待准备，本轮不自动下载、升级锁文件或启用服务。

用户要求参考旧配置后，只读提取了`config/server.toml`与27个`.d`片段、`config/settings.toml`中的必要非敏感字段，没有归档完整私有配置或凭据。base/fragments中的major和minor均为DeepSeek `deepseek-v4-flash`，sentence_expression为Gemini 2.5 Flash，embedding为OpenAI `text-embedding-3-small`／1536；Whisper为旧localhost:8080基址和large-v3说明标签，TTS旧localhost:9880/tts但参考与权重路径为空，没有Easy-Turn配置块。persisted settings另有local embedding／Nomic768、不同minor模型以及STT/TTS noop等历史值。**这不是一份已确认的新方案，也不能仅按其中任一文件宣称有效配置**；下一轮先建立任务独立配置并按实际注册项／应用时序核对，避免直接启动旧配置触发不同供应商或不完整TTS配置。具体字段与路径由[配置交接](../refactor/handoffs/2026-10-17-model-preflight/config-HANDOFF.md)记录，指定模型目录盘点见[验收交接](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。

已找到的本地候选如下；这里只记录checkout／磁盘配置／文件存在，未执行或证明实际loaded身份：

| 组件 | 本地盘点 | 后续具体缺口 |
| --- | --- | --- |
| Easy-Turn | `Developer/Easy-Turn`和项目`run/models/easy-turn-source`的HEAD均为钉住的`fe175eaa…`；checkpoint字节数相符，Qwen小manifest修订相符；三个官方PCM的SHA匹配 | 未重新hash大权重或检查模型checkout全部dirty状态，运行版本／端点未验 |
| GPT-SoVITS | `Developer/GPT-SoVITS` HEAD=`08d627c…`，现有配置为CPU／v2ProPlus，指向`Juno-e30.ckpt`＋`v2Pro/Juno_e16_s80.pth`，文件均存在 | 项目未选参考音频与匹配文本；六个demo WAV存在但未读取／听取。`go-api.command`默认`0.0.0.0`，未来开发启动需明确loopback参数 |
| Whisper | 通过项目安装脚本定位至`/Users/ke-qiao/.local/share/alia/whisper.cpp`，HEAD=`968eebe…`；已有server二进制与`ggml-large-v3-turbo.bin` | 实际目录不在项目内；旧配置`large-v3`仅说明标签，需按实际启动参数确认turbo，未执行二进制 |

这些发现使后续可复用已有资产，不要求重新安装或容器化模型。只有需要PG等隔离资源时再使用必要容器，开发模型服务的具体启动参数与资源归属按有限验收步骤确定。

## 下一轮有界执行门槛

1. 基于本轮答复与目录／旧配置盘点，补齐待定的模型ID、协议和素材选择，再形成可审阅的任务配置及调用清单。包含所有可触发的角色、embedding、TTS初始化切权重、配置覆盖层和SQL政策身份；未选服务不能沿用旧配置或真实示例默认值。
2. 确认服务和数据归属、允许的启动/停止及有限请求范围后，分别验证真实组件。保留默认`health_probe_enabled=false`和shadow；health作为状态记录，真实输出和服务端加载身份单独核对。请求失败先保留和定位，不改原期限/并发求绿。
3. 复用既有Easy-Turn组件/live测试与固定离线PCM；正弦或静音PCM仅用于传输，不作语义准确性样本。两项现有live测试只证明Easy-Turn，并没有真实LLM或TTS生成。
4. 组件明确后，再补现有Generation/Delivery入口的LLM→TTS组合证据，记录Response/Context/output身份、模型文本和音频字节，输出仅存私有文件、不播放。具体入口与已有期限见[验收HANDOFF](../refactor/handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md#下一轮可执行顺序本轮未执行)，本轮不预建框架。
5. 保存失败分母、源码／模型／样本身份、冷启动与后续延迟、请求和服务原owner清理证据。不能把HTTP取消或超时当作native推理已经结束，也不终止不属于本次验收的共享服务。

## 保留的后续范围

真实模型链尚未验收；Mac当前完整发布矩阵、服务账户部署和长稳也未由Linux证据替代。R02仍缺R1板/固件、输出链与AEC电气资料，R03在用户恢复前保持暂停，R05等待软件与现场门槛。历史V01根因仍open，default独立第三LLM profile的校验/实际选择差异只记录为后续需明确的边界，本轮不改实现。前端与通用Embedded适配仍延期；R01—R06仍2/6，不换算整体百分比。

本轮状态及下一次恢复入口见[checklist](../refactor/checklists/model-preflight-2026-10-17.md)和[恢复检查点](../refactor/resume-checkpoint-model-preflight-2026-10-17.md)。
