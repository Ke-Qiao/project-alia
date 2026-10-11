# 配置目录持久性与发布恢复：2026-10-11

承接 `7165ad6`（Health 轮记录提交），分支 `codex/architecture-refactor`。本轮只清理 Server 配置 base/fragments 的 setup 发布及恢复，保持 HTTP API、Host/Origin/JSON 限制、loopback、CLI 安装方式、当前政策和权限边界。三个 subagent 分别负责持久化、完成流程与真实 HTTP 回归，Root 负责 loader/discovery、交叉整合、唯一 Cargo/PG/Docker 验证窗口和提交。

## 已有实现与缺口

旧 writer 已做有效配置/role 校验、私有候选写入、文件 sync 和原子 rename（CLI 用不覆盖的 hard link）。旧完成门已在一个 watch 写锁内协调 HTTP、CLI 和取消。本轮保留这些责任，不另建并行写入口。

缺口是父目录、新 fragments 及祖先目录链没有同步，且发布后的错误与发布前错误没有区别。若只在末尾添加目录 sync，错误会重新开放 setup；CLI 的通用失败清理还可能删去已发布主文件依赖的 fragments。重启发现只检查主文件存在，也无法辨别此前提交是否已经确认持久化。

## 新归属与发布协议

发布和恢复归 `config/publication.rs`，OOBE 只保留嵌入模板、交互与完成状态。删除 OOBE 旧 writer、`release_example_config`、`finish_setup` 和 `SetupResult`，没有兼容发布旁路。正常无恢复记录的加载继续只读，避免要求已有只读配置目录提供同步权限。

一次发布按以下顺序完成：

1. 校验 HTTP 实际 base+fragments 或 CLI 样例合并结果；尚未验证的请求不建目录。
2. 准备私有候选和 CLI 新分片（拒绝覆盖现有分片树），同步文件、从内向外的目录树及完整 canonical 祖先目录链，核对实际布局。
3. 把仅含原意图、受限路径和文件快照的 descriptor 写入私有临时文件并同步，以 exclusive hard link 发布为固定相邻恢复记录，再同步目录。记录不含配置值或密钥。
4. HTTP 原子替换主文件；CLI 用 hard link 保持不覆盖语义。重新核对内容和文件身份，确认主文件及目录同步。
5. 数据持久化确认后才删除候选和恢复记录，最后同步目录；只有全部成功才能进入 Ready。

V1 库回归通过后，交叉复核发现早期失败留下的空父目录会让下一次保存错误缩短同步范围。V2 删除基于“本次新建”的祖先计数，保存/恢复始终同步完整 canonical 父链，新增失败后再次保存的回归；普通无 marker 加载仍不增加同步。保存与恢复若没有所需目录访问或同步能力会明确失败，不修改权限或提权。

固定记录、候选名称和相对 fragment 路径都需验证，恢复不接受任意外部绝对路径或父目录跳转。现有 base symlink 仍解析到原目标，不把正常 loader 改成新的路径政策。恢复只确认已捕获意图，文件被观察到改变或丢失时明确失败并保留现场，不盲覆盖。

## 错误、重试与取消

| 状态 | 文件处理 | 后续行为 |
| --- | --- | --- |
| 发布记录建立前确定失败 | 只清理本次拥有的候选/CLI 新树；保留旧主文件与 operator fragments | 可修正输入再保存 |
| 记录发布后任意失败 | 保留候选、主文件和依赖，返回 typed PendingPublication | 仅同目标/内容/安装方式重试 |
| 恢复发现快照改变或缺失 | 保留现场 | 返回 unknown，需操作者核对 |
| 恢复记录清除后最后目录 sync 失败 | 数据已确认，但本次完成仍未确认 | 内存 descriptor 继续核对并补 sync，不把“无 marker”当作重试成功 |
| 取消或退出 | 封闭本次 setup，不删除 pending 文件 | 再启动选择同一配置路径恢复 |

HTTP 仍返回原 `success/path/error?` JSON 和原状态码；HTTP 200 必须检查 success。`SetupCompletion` 唯一 watch 状态为 Idle / Pending / Ready / Cancelled，没有第二个锁或后台重试。Pending 不触发启动或结束 setup。CLI `c` 可以重试原安装，`q`/Ctrl-C 保留文件退出。Ready 后保留原 2 秒响应排空与连接清理，取消后的迟到 HTTP body 不能重新发布。

启动发现也识别所选候选路径的 pending 记录，因此 marker 存在而 base 尚未建立时不会进入另一次 setup。`ServerConfig::load` 只对该记录执行恢复，校验成功后进入正常加载。自定义路径重启仍需原 `ALIA_CONFIG_PATH`，不扫描整台机器寻找历史安装。

## 验证范围

故障注入限定每个测试自己的临时配置目标，不使用全局环境切换或放宽生产期限。覆盖文件/分片同步、目录与祖先顺序、记录与主文件发布、记录删除、相同意图重试、不同意图拒绝、真实 HTTP、取消和旧完成 owner 丢弃后的实际 loader 恢复。没有接触真实用户配置或外部模型。

## 最终验证与证据

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

| 命令 | 平台 | 结果 / 秒 |
| --- | --- | --- |
| `cargo test --workspace --offline --locked` | macOS ARM64 | 1108/0/5 / 262.25 |
| `cargo test -p alia-server --offline --locked` | Linux ARM64 | 826/0/5 / 365.031 |
| `cargo check -p alia-server --no-default-features --all-targets --offline --locked` | macOS ARM64 | exit 0 / 1.67 |
| 同上 | Linux ARM64 | exit 0 / 23.906 |

V1 Server lib 为 **357/0/3**（96.29 秒），随后主动复核发现早期失败遗留父目录的重试缺口。V2 删除祖先计数并新增针对回归，macOS workspace 为 1108/0/5。V2 Linux 第一次运行中途计数为 480/1/3，在既有 `control_ack_loss_is_observed_from_original_device_command` 的立即 `!is_held()` 断言失败（exit 101），同一二进制默认并行 9 项随后全部通过（13.445 秒）；这次复查不能抹去首次失败。代码顺序证明数据库 receipt 可见先于本地 gate 解除，V3 仅让该测试在共同的原 3 秒期限内等待两者，保留最终断言、原 32 秒场景期限和 cleanup，不改生产 Playback。最终验收采用 V3 两端完整回归。V1 Linux 仅记录环境；V2 Linux 无默认检查因前置失败没有执行。所有版本清单与原始日志保留，不能把部分运行累计为最终通过数字。

macOS 26.6.2 ARM64 / Debian 12 ARM64，Rust 1.95.0；仅复用已有离线依赖、native archive 与容器镜像。macOS 标准库目录 sync 的独立探针通过。定向阶段与全量包含重复用例，不相加作为独立测试数；5 个 ignored 仍按原环境门槛保留。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

原始命令、日志、冻结清单、Git 输入核对与资源状态见 [证据索引](handoffs/2026-10-11-config-durability/evidence-index.json)。

未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。

## 保留的边界与下一项

本轮证明的是规定同步调用、明确失败状态及恢复软件行为；没有用进程 kill 或故障注入代替真实断电。存储设备是否兑现 flush、服务账户/路径部署权限仍需实际环境验证。同 UID 的其他进程不能被这个完成门锁住；不支持在发布期间并行修改 fragments/symlink。

恢复记录建立前的崩溃可能留下无法确认所有权的临时文件或 CLI 新树；不得因名称相似就自动删除。已有配置读取、runtime `settings.toml` 写入分别保持当前职责。下一软件候选是先审计 `SettingsRegistry::save` 与 PATCH 应用/持久化顺序，定义失败是否回滚/保留的契约，再在授权范围补缺口，不扩展本轮 descriptor 成通用事务框架。

L09—L12 仍 4/4，R01—R06 仍 2/6。V01、R02 AEC、R03 反馈、Pi/24h/R05、R06 其他平台以及前端等暂缓项继续见 [延期台账](deferred.md)。
