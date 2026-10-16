# 恢复检查点：当前版本 Linux 软件发布验收

2026-10-17；分支 `codex/architecture-refactor`，基线 `da3eca2fe6853f104dd558051aebf92dd62f067b`，应用源码仍为 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。本轮 **10/10 完成**；Linux 构建/测试/release 链接和独立证据审查均通过。正常文档提交见本轮 Git 记录，应用源码未变。本轮没有生产、测试、配置或依赖修改。

先读[checklist](checklists/linux-release-2026-10-17.md)和[四份 HANDOFF](handoffs/2026-10-17-linux-release/README.md)。保持默认 shadow、政策、执行批准、精确取消与资源清理、原测试期限/并发；R03 仍暂停，无真实硬件或新平台授权。

## 已完成及可恢复证据

- 同一冻结 677 输入，在既有 Debian 12 / Linux ARM64 / Rust 1.95 环境一次默认完整 workspace：**1285 passed / 0 failed / 5 既有 ignored / 0 filtered**，67 Running + 5 Doc-tests，483.737 秒。与上一轮 Mac 1277 的差异仅为七项 ALSA 软件 null 与一项 PCM 名称测试，其他目标和 ignored 一致。
- Server 与 Embedded 分别 no-default all-targets：21.055 / 6.163 秒，均 exit 0；两端 default release：115.452 秒，exit 0。真实 ELF/loader/ldd 均通过，Embedded 170 个定义 Sherpa 符号及安全 help 正常。没有重复测试求绿或改期限并发。
- manifest `c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`；677 源输入与基线 Git、最终宿主和上一轮 Mac 同字节。所有阶段 drift=[]，既有 Sherpa archive 和实际 15 个静态库在构建前后均匹配。
- Server release SHA-256 `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330`；Embedded `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003`。独立副本在 `run/validation-20261017-linux-release/artifacts/`。
- 测试结束 11.165 秒后同库 0 schema / 0 other client，关闭前再检查 0/0；本轮两个容器、PG 卷、internal network 已删除，共享 astrbot/旧资源保留。没有待继续运行的 Cargo 或任务容器。
- [证据索引](handoffs/2026-10-17-linux-release/evidence-index.json)归档 50 份字节一致原件，共 721154 字节；ignored run 保留源码快照、缓存、二进制和原布局脚本。不得因路径仍存在便复用旧结果给新源码背书。

## 下一软件出口 checklist（不是本轮已完成项）

- [ ] 先核对 Git 状态与当前源码身份，再审查既有全 noop staging 模板是否符合当前配置契约及禁用设备要求。
- [ ] 在已确认平台和独立配置/目录/数据库上，做当前 release 的真正进程启动、关闭、重启和资源回收演练；Server `--help` 会正常启动，不能拿来代替安全检查。
- [ ] 依据已有配置保存/恢复契约，有界核对实际进程恢复及部署说明；先补确认的缺口，不预建发布框架。
- [ ] 用户确认实际模型服务端点、版本及运行方式后，再验证真实模型链；当前纯软件通过不代替模型验收。
- [ ] 汇总受测源码、二进制、配置模板与恢复说明，明确软件发布出口；历史 V01 根因继续有界跟踪，不由本轮成功关闭。

R02 的实际 R1/AEC 电气、输出链、固件和声学资料仍缺；R03 保持暂停。R05 等软件与现场门槛齐备后进行，前端和通用 Embedded 适配继续延期。六轮仍 **2/6**，不以本轮局部验收推算项目整体百分比。
