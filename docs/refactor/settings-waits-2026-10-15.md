# Settings 慢模型、额度等待与关闭边界

日期：2026-10-15；基线 `e89d34c`。本包按实际复现补缺口，保留现有 Settings/Privacy API、当前政策、管理员准入和部分成功语义。跟踪见 [checklist](checklists/settings-waits-2026-10-15.md)。

## 范围与实际缺口

OpenAI、Anthropic、Gemini 的请求 helper 原先在等待模型 HTTP 响应期间持有 provider 配置读锁。Settings 顺序 apply 需要同一配置写锁，因而既能被慢模型拖住，又会持有全局 update gate 阻塞后续 GET/PATCH/privacy。三个真实 loopback RED 均在“Settings 应在模型仍等待时完成”断言失败。

现在每个请求在进入网络前固定一份完整配置。OpenAI 的 api_family、endpoint、model、credential 使用同一快照，包括 Responses 工具流的原非流式降级；Anthropic/Gemini 的普通与流式 helper 同样释放配置锁。已开始请求使用其原配置，后续请求使用新配置；没有新增任务、全局模型 timeout 或撤销已发请求的行为。

SoVITS 权重设置原先先等待 4096 字节错误体额度，随后才开始 reqwest 的单请求期限。隔离子进程耗尽真实全局额度（只占 permits、不分配 PCM）后，原请求超过期限仍 Pending。修复将原 timeout 包住整次额度准入、HTTP 和错误体读取。每次权重请求仍使用现有配置期限；两次权重调用仍顺序执行，不能把这个数字当作整个 Settings 的上界。远端失败继续 warning，本地配置与既有 applied 返回不回滚。

## 取消、关闭与部分成功

| 等待或阶段 | 所有者及当前行为 | 证据与边界 |
| --- | --- | --- |
| LLM 网络/响应体 | 调用者拥有请求或返回流，配置锁已释放 | 本地 peer 见证请求仍挂起时 apply 完成，drop 后 EOF/reset；不代表远端撤销已发生计算 |
| SoVITS 权重额度/网络 | borrowed future 与 reservation 同生命周期；单请求 timeout 包含准入 | 真实 permits 排队、虚拟时钟期限与取消后全额重取；没有后台继续完成任务 |
| Settings update gate | handler 持有 Mutex guard，真正 future drop 时释放 | 原 Registry 入队/持锁取消测试与本轮真实 HTTP/SQL 关闭证据分别记账 |
| SQL 政策确认附近 | SQL CAS/actor 检查保留；确认超时已有 2 秒预算 | 已提交或确认不确定不能说成“全回滚”，需要重读版本 |
| desired / 顺序 module apply | 已修改 desired 与已完成效果保留，同值显式重试继续 apply/save | HTTP apply 探针先产生效果再等待，RAII sentinel 证明真正 drop |
| 文件发布 | save 等锁可取消；进入同步文件调用后不能被 Tokio 抢占 | 原子旧/新完整文件和原发布前/后错误语义保持，真实断电不在本轮验证范围 |

初始静态审查怀疑 HTTP/1 pipeline 缓冲会挡住取消，但原实现实际通过：Hyper 除读路径外也 poll flush，已有 Connection::poll_flush 观察取消。保留真实结果与修正理由，没有新增 API 包装、全局回滚或整请求期限。不能仅从客户端任务结束推断 server handler 已释放；测试必须见证实际 Drop、锁和 Runtime owner。

任意扩展模块、同步阻塞工作及整个 Settings 的完成时间仍无统一 SLA。取消不撤销已提交 SQL、已生效本地模块或远端已接受的权重切换。本包不将有限回归推广为所有协议/所有扩展的关闭保证。

## 验证与恢复

实际执行结果、冻结输入、隔离数据库和清理记录在 [四份 HANDOFF](handoffs/2026-10-15-settings-waits/README.md)。本轮只使用本机缓存依赖、loopback 模型 mock 和独立 PostgreSQL；用户已明确授权共享 OrbStack 启动及隔离测试库，收尾只删除本轮数据库，共享服务保留。

R03 仍暂停；R02 硬件 AEC 优先路线、厂商/输出链资料缺口及真实录放音门槛不变。V01、可信部署账户/文件系统、真实模型与 R05 整机长稳仍单独跟踪；本包没有跨平台认证或硬件证据。

本包最终 **10/10**，源码 `4e99134`。Server全目标868/0/5、最终provider51/0/0和无默认all-targets检查通过；整包到最终提交仅一处cfg换行，身份差异见[证据索引](handoffs/2026-10-15-settings-waits/evidence-index.json)。669个最终输入匹配提交；本轮隔离数据库已删除。
