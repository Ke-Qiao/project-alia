# 当前检查点：2026-10-11 /health 清理完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，没有待恢复的 Cargo/PG 作业。

依次读取 [技术说明](health-2026-10-11.md)、[checklist](checklists/health-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-health/README.md) 和 [证据索引](handoffs/2026-10-11-health/evidence-index.json)，然后核对 Git branch/status/log。

源码 `29420a57d6558d90c36098c061655dff40eb72f4`，V2 manifest `9902508f7743bbbae5d447fec76c753d9f8910b0b1b019ef3ea5f801d03a21b0`，646/646 Git blob 一致。

整体 health 改为 6 秒绝对预算、最多 8 项并发，同 Registry 一轮准入；重叠请求立即返回原字段的 busy 降级，未启动额外 probe。实际同实例角色共享结果，同名异实例独立。配置同 profile 的 LLM 角色共用 Arc 与 init/start/settings/stop owner；显式 override 保持独立，失败回滚保持。Router 只检查本地模式和依赖绑定；OpenAI 既有实现复用，Gemini 补 5 秒期限、锁释放与安全错误。

已完成结果保留，未完成项超时或停止降级；调用者 drop／stop 不留下无主 probe，停止后不再准入。API、默认 probe=false、当前政策保持，无新增依赖、feature、配置、平台或硬件动作。没有新增结果缓存或深度诊断端点。

macOS workspace **1094/0/5**；Linux ARM64 Server 默认并行完整回归 **812/0/5**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。

**下一软件任务：配置目录持久性。先核对 base 文件与 fragments 的创建、同步和原子发布调用链，定义发布后父目录 sync 失败的未知结果、目录树/祖先同步和恢复规则，再补必要实现与隔离文件系统回归。保持当前 API、政策、权限与已有依赖；真实断电不能用进程 kill 代替。本轮未实施这项。**

可信本机文件系统的部署账户、权限和路径仍开放；原 V01、R02 AEC、R03 反馈、Pi/24h/R05、其他平台/R06 与用户暂缓项见 [延期台账](deferred.md)。本轮没有修改常规模型生成中的原有配置锁等待语义；不同 profile 指向同 endpoint 仍独立。

所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 0）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。

未推送／未建 PR，本轮未使用 reset 卡。未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。
