# Classifier 与运行 Skills 目录衔接

基线 `4e4d59529a4beef21b022eb36b5dc89cb990a497`，分支 `codex/architecture-refactor`。本轮 **10/10 完成**；L09—L12 保持 4/4，R01—R06 保持 2/6，不表示项目完成百分比。

- [x] **C01 审计**：恢复四份 HANDOFF，核对静态模板、RouterInput、名称校验、generation/prompt/缓存及调用者；三个 subagent 分工。
- [x] **C02 契约**：统一 live candidate、分类期间变化及缓存关联语义；明确原路由模式、政策与资源边界。
- [x] **C03 Registry**：当前 enabled 候选的完整有序投影，保持索引失败和提交所有权保证。
- [x] **C04 Router**：移除静态技能表与专名捷径，模型只能选择此次实际候选，保留 major-only / minor-only / both 的回退政策。
- [x] **C05 Generation**：实际接线、候选与上下文一致性、分类期间失效的正常退化；不增加工具权限。
- [x] **C06 回归**：真实 HTTP CRUD→Interaction→分类→回答、未知技能、在途变化及政策回归。
- [x] **C07 macOS**：冻结最终源码，完成新增回归、workspace 和无默认全目标验证。
- [x] **C08 Linux**：同源 ARM64 Server 默认并行回归与无默认全目标检查。
- [x] **C09 交接**：技术说明、四份 HANDOFF、证据、延期台账和恢复检查点。
- [x] **C10 提交收尾**：正常源码与文档提交，源码/证据身份核对，任务资源停止和工作区干净。

默认 Shadow、工具全局可见性、Execution 批准、原路由模式和 timeout 保持；不扩展依赖、feature、平台、模型或硬件范围，不访问用户 5433。

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

所有 Cargo 阶段结束；关闭前其他数据库连接与测试 schema 均为零，任务 Linux holder 与 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 进程在 docker stop 后 exit 137，验证命令均 exit 0；未把容器停止码当作测试结果。缓存与只读快照保留在 ignored `run/validation-20261011-classifier/`。
