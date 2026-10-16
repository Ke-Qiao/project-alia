# 恢复检查点：软件发布资料与真实模型链前置

**后续接续：** 当前转入[插件契约与信任边界检查点](resume-checkpoint-plugin-contract-2026-10-17.md)。下列模型精确ID／素材及运行验收待定项继续保留；插件文档轮次不关闭这些出口，也不构成启动服务许可。

2026-10-17；分支 `codex/architecture-refactor`，资料起点 `6a8b1133cf00c4ff6abe9ddc6efc8047029551a5`，已验应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`。本轮 **10/10**：发布资料与三组答复收口，明确未定项保留；没有真实模型运行。本轮只有文档与两行示例注释删除，随正常提交保存。

恢复先读[本轮checklist](checklists/model-preflight-2026-10-17.md)、[四份HANDOFF](handoffs/2026-10-17-model-preflight/README.md)、[发布记录](../operations/software-release.md)。先核对当前Git与本轮提交，不把旧全量结果说成新源码验收。

## 已完成

- 三个subagent分别审计配置契约、真实模型验收入口和发布证据；Root汇总。修正不存在的`speech.enabled`、已移除的STT纠正角色注释和错误的Memory启动降级说明；没有新增生产实现、测试框架或依赖。
- 起点678输入与受测manifest完全一致，两个Linux产物与148份旧证据只读复核一致。本轮最终677项字节一致，唯一变化为Server示例删除两行注释；TOML解析语义相同，Rust／Cargo／锁文件不变。旧manifest SHA `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`仍代表旧受测输入。
- 当前Linux `6c62c84` workspace1287/0/5/0filtered、release／native链接及双轮noop正常启停／恢复已在上轮通过；当前Mac仅两项定向通过。旧Mac全量1277/0/5属于`4e2968f`。产物路径／hash见发布记录；本轮未重复运行这些批次。
- GPT-SoVITS初始化可能切换远端权重，失败仅warn继续；health成功不证明实际加载身份。LLM默认probe=false的Degraded不应套用noop全healthy标准。两个既有live目标只验证Easy-Turn，并没有真实LLM/TTS组合生成。
- 核对了配置层、各角色与embedding、当前默认角色选择边界和已有请求／清理预算，整理下一阶段步骤。收到用户要求后只读核对指定模型目录与旧私有配置的必要字段；未输出或归档凭据／完整配置，未探测端点、下载资产、启动服务、连接Pi或启用硬件。

## 用户已确认与明确待定

1. 对话与embedding具体方案待定；拟使用`https://botcf.com/v1`测试。没有访问该地址，不假定它支持所需model、API family、tools／SSE／JSON或embedding；不重复索问用户已明确待定的同一资料。
2. Easy-Turn与GPT-SoVITS在Developer目录，TTS使用Juno权重，Whisper在项目内，用户要求参考旧配置。只读目录盘点见[验收交接](handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)，文件存在／git版本不代表实际已加载的模型或服务可用。
3. 当前属于开发验证，无需正式服务账户／托管方案；沿用当前账户和任务独立目录，必要时容器隔离。当前Mac目标不变，本轮未创建容器；正式部署与R03／现场另行验收。

MP10已收到并登记答复，资料确认完成，实模执行仍未就绪。旧`config/server.toml`与27片段包含DeepSeek双聊天、Gemini表达、OpenAI1536 embedding、Whisper与不完整TTS配置；`config/settings.toml`另有local embedding／Nomic768、不同minor模型及STT/TTS noop。不要直接复用为本次配置，先核对初始化和注册设置的应用次序。配置细节见[配置交接](handoffs/2026-10-17-model-preflight/config-HANDOFF.md)，不把旧值默认当作用户新选型。**接收到地址不等于本轮自动探测或启动服务。**

本地已找到Easy-Turn钉住源码与checkpoint、三个SHA匹配的官方PCM；GPT-SoVITS为`08d627c…`，现有CPU/v2ProPlus配置指向Juno-e30与Juno_e16_s80，未选参考WAV／文本；脚本默认监听全部接口，后续开发须显式loopback。Whisper按项目安装脚本实际位于`/Users/ke-qiao/.local/share/alia/whisper.cpp`，`968eebe…`，模型文件为large-v3-turbo，和旧配置说明字段large-v3不同。大权重未重hash／加载，服务均未探测或启动。

## 下一轮顺序

- [ ] 在已登记的MP10答复基础上补齐精确模型／协议／Juno素材选择；核对指定受控路径，避免归档完整settings／凭据。
- [ ] 确定实际配置覆盖层、SQL政策身份、provider初始化副作用和资源owner；不为模型验收改变政策或执行批准。
- [ ] 根据明确范围补Mac发布与模型组件证据；不拿Linux二进制替代Mac产物，不安装缺失依赖或自动下载权重。
- [ ] 复用两个Easy-Turn live目标的固定PCM／原期限／默认并发；小PCM摘要本轮已匹配，模型大权重／环境的完整身份与实际加载状态仍待验。
- [ ] 再完成现有Generation／Delivery的真实LLM→TTS组合，音频仅写私有文件、不播放；保存模型／源码／样本身份、失败分母、延迟与清理结果。

具体步骤、现有命令和预算见[acceptance-HANDOFF](handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。不要重建通用框架，不靠提高期限、减少并发或无变化重复测试求绿。

默认shadow、probe=false、当前政策／权限、精确取消与原owner清理不变。R03暂停，R02资料缺口与R05现场门槛继续保留；历史V01根因open，default独立第三LLM profile的校验／实际选择差异仅记录，未修实现。R01—R06仍 **2/6**。本轮未证明真实模型链、Mac/Pi独立部署、正常绑定恢复、crash／断电、硬件、MPS或长稳通过。
