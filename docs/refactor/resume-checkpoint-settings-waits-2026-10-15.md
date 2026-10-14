# 当前恢复检查点：Settings 等待与关闭收尾

日期：2026-10-15；分支 `codex/architecture-refactor`；基线 `e89d34c`；源码 `4e9913414c68b4a78b52c472a04d5a2139785430`。本软件包 **10/10**，三个subagent model **7/7**、audio **6/6**、review **7/7**，均已完成交接并停止写入。R01—R06 仍 **2/6**；R03继续按用户要求暂停。

先读 [checklist](checklists/settings-waits-2026-10-15.md)、[设计/边界](settings-waits-2026-10-15.md)及[四份HANDOFF](handoffs/2026-10-15-settings-waits/README.md)，再核对 Git branch/status/log；不要重复实施已验收的软件修复。

## 本轮结果

- OpenAI/Anthropic/Gemini请求改为一次完整配置快照，不持配置读锁等待模型。OpenAI协议家族与endpoint/model/key同代，流和取消owner不变。
- SoVITS权重请求原期限包含额度准入、HTTP和错误体；保持本地修改、远端warning和部分成功，不新增整个Settings总期限。
- 原Runtime的HTTP/1 pipeline关闭基线已通过，静态推测被撤回，因此API/Runtime没有新增取消包装。三项真实HTTP验证覆盖shutdown、实际TCP断开后的同值重试、privacy共享policy锁等待时关闭。
- SQL/desired/已产生模块效果不因取消回滚，未进入保存时磁盘保留完整旧文件；同值显式重试继续apply/save。默认shadow、当前政策、管理员准入和执行批准保持。

## 验证与交付身份

macOS26.6.2 ARM64、Rust1.95.0，本机缓存依赖和隔离PG。Server全目标 **868通过/0失败/5跳过**，50个Cargo目标，默认并行与原期限；其中runtime81/0/0。5跳过为真实PTY3和真实Easy-Turn2。本轮8个新增测试已包含在868内，不重复累加provider51、关闭3或子进程重入。

完整测试的green manifest为 `2b4524993978bcc30c675b4ffc8bd2b2757e8b2250ebdddbcc912932d9fd105e`。随后仅将llm/mod.rs的cfg属性换行，精确转换和其它668输入不变已校验；最终provider51/0/0、无默认all-targets检查和9个改动Rust格式检查通过。final manifest为 `313671a7cee06b5485bd1f2eb9e895cc23ee347e0ced02eb87ae758e6b7b24ce`，669个提交blob匹配，无额外/缺失。证据见[索引](handoffs/2026-10-15-settings-waits/evidence-index.json)。没有把格式后版本冒充再次全量运行。

用户批准共享OrbStack启动及独立测试库；实际start返回already running。task DB已删除，未停止其它共享服务或OrbStack。没有本轮监听器、测试数据库或后台worker待清理；原始ignored运行目录保留用于追溯。

## 下一次继续的范围

1. 若继续软件收尾，先按新指令选择已有待办；V01有界复现/阶段证据仍开放，本轮全量通过不能关闭历史根因。不要再自动衍生Settings统一超时或框架。
2. 同步文件I/O不可被Tokio取消抢占；任意扩展/真实模型/代理或H2的取消行为、可信服务账户/目录/文件系统、staging与部署恢复仍待对应验证。本轮HTTP证据只覆盖当前直接HTTP/1；纯GET/PATCH update gate排队没有独立HTTP阶段见证，已有Registry局部取消和静态顺序证据保留。
3. R02硬件AEC优先已确认，仍等R1板/固件、厂商附件、扬声器/功放和参考电气/模式/延迟。资料未齐不预建AEC框架，不启用真实录放音。
4. R03后续保持暂停；停止保持、校准后归中和双端本地信任的软件已完成项保留。未确定机构/供电/OE/通道/校准之前不执行器件动作。
5. R05尚未进入整机验收；前端、通用Embedded适配、端上模型/同机组合及额外平台沿用[延期台账](deferred.md)。真实音频/机械/模型/长稳结果不能由本次软件测试替代。
