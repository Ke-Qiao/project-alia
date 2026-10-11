# Storage HANDOFF — 2026-10-11 运行时 settings 持久化

分工：`skills_registry`；基线 `1d6ef37`，共享分支 `codex/architecture-refactor`。已读取上一轮 config-durability 检查点和 HANDOFF。本文是 storage 分工的交接，不代表整轮最终验收。

## 当前进度与 checklist

- [x] 审计 SettingsRegistry 读写、所有生产调用者、启动加载和已有重试语义。
- [x] 与 Root / HTTP agent 对齐保留 desired/live 部分成功及显式重试，不引入跨域回滚。
- [x] 实现同 Registry 保存准入，取得 gate 后再读取快照；I/O 期间释放 registry 读锁。
- [x] 实现 settings 私有原子 writer、文件及完整 canonical 父链同步、错误分类与候选清理。
- [x] 保留有效/悬空 terminal symlink 的跟随行为、既有目标写权限检查和 POSIX mode；拒绝 hardlink 与无法保留的 owner/group。
- [x] 读取只接受完整解析结果，保留 schema；missing 包括 dangling target 保持原成功返回语义，其他 I/O 错误不再被 exists() 掩盖。
- [x] 编写 7 项内部回归和仅内部使用的按目标隔离同步屏障/故障事件。
- [x] 完成 scoped rustfmt、独立读回和跨 agent 接口审查；源码停写。
- [x] Root 冻结源码的实际编译、定向与完整回归。
- [x] Root 两端无默认 feature / 静态守卫 / 资源清理。
- [x] Root 最终 checklist、恢复检查点、证据和提交。

实现 **8/8**；最终验收与提交现为 **3/3**；实施阶段待验收状态由下方 Root 最终记录关闭。

## 变更文件与实际调用边界

拥有 `packages/alia-server/src/settings/registry.rs`、`settings/storage.rs`、`settings/storage/tests.rs`、必要的 `settings/mod.rs` 导出和本文。没有改 HTTP、privacy、bootstrap、公共 integration tests 或配置域 publication。

原生产持久化调用者只有 Settings PATCH 和旧 Memory privacy PATCH。后者绕过 updates gate，修改的 key 却全部被 save 的 SQL-policy 过滤规则排除，且吞掉保存错误；HTTP owner 在本轮将其迁移到共同 SQL 政策 CAS 和 admin 权限，已经移除其 save 旁路。启动 builder 加载文件、设置路径，之后用 SQL policy 替换投影；该启动政策顺序保持。

无调用的 set/register/remove/plugin 支持等公开接口没有顺手删除。`save` / `load_persisted` / `set_persist_path` 签名保持；公开新增 `SettingsSaveError::was_published()` 供 API 精确分类。非政策 key、null 过滤和已有 schema 保留规则没有扩张。

## 保存与失败契约

SettingsRegistry 是期望设置的内存视图，模块应用和文件保存不是跨 SQL/模块/文件的原子事务。保留现有部分成功语义：SQL 已提交或模块已生效后，不因文件错误伪造回滚；显式同值 PATCH 仍重新 apply 并保存。

保存先取得 Registry 共享的 persistence gate，再克隆目标路径及待持久化 values；读锁随后释放。排队的保存不会提前持有旧快照，`load_persisted` 和 `set_persist_path` 也使用同一 gate。现有 API updates gate 仍负责整次请求的政策/应用顺序。

私有 writer 在目标相邻目录创建独占临时文件，以 Unix 0600 写完内容，再恢复既有目标的 mode，sync 文件、关闭它、原子 rename 到真实目标，最后同步完整 canonical 父链直到根目录。只在 rename 成功后标记 `was_published=true`：

- `false`：无配置路径、序列化、目标检查、准备/文件同步或 rename 失败，旧目标未被本次发布替换；候选清理错误也作为失败附带报告。
- `true`：rename 已完成，后续父链同步失败；文件可见且完整，但持久性未确认，API 必须明确错误并要求 reload/retry。

不引入 setup 的 pending descriptor 或 journal。显式重试重新保存当前完整期望快照；保存报错后的重启可能加载旧或新完整文件，不承诺自动继续未确认请求。普通 loader 不执行同步，先完整读取/解析，最后才在一个写锁内合并，错误不会部分污染内存或 schema。

## 路径、权限和资源边界

- 有效 terminal symlink 解析到真实目标；悬空 terminal symlink 在其目标父目录已存在时创建目标，保留 symlink 本身。目标父目录缺失仍报错，不新增该树；循环链接明确拒绝。
- 在 write-open 前拒绝非 regular 目标，避免 FIFO 的写打开阻塞。对既有 regular 文件先执行不截断的 write-open，保留原本的写权限检查，再检查实际打开文件的类型。
- Unix 新文件为 0600；既有文件保留 POSIX mode。原子替换会更换 inode，故现有 `nlink > 1` 或 candidate 与旧文件 uid/gid 不一致时保守拒绝，不 chown、不提权。
- ACL、xattr 等 inode 扩展属性没有复制，也不能据 POSIX mode 测试声称完全保留；部署使用这些属性的场景须另行审查。权限/文件系统不能打开或同步父链时不会忽略失败。
- 没有后台保存任务，也没有同步 I/O 中间的 await；普通请求取消不留下分离的 writer。保存前半段仍可在等待 gate/读锁时取消，尚未进入文件写入。
- 候选仅在独占创建成功后归本次操作所有；rename 前错误会尝试清理，清理失败仍明确报错。进程崩溃可留下私有临时文件，加载不会扫描、认领或自动删除它们。
- 不提供其他进程的文件系统锁或 CAS；不声称防同账户在检查与 rename 之间恶意换路径。正常的本机部署文件系统边界与前轮一致。

## 内部回归与验证状态

7 项内部回归已写入：

1. 准备阶段 FileSync 故障保留旧完整快照并清理候选。
2. rename 后目录同步故障分类为 unknown；显式同值重试依序完成文件同步、发布和全部祖先同步。
3. 早期失败留下新父目录后，下一次保存仍同步完整祖先链。
4. 非 regular 目录目标在 write-open 前拒绝，不创建候选。
5. Unix 新文件 0600、已有 mode/uid/gid 保持、hardlink 拒绝且其他链接内容不变。
6. 有效和悬空 symlink 跟随、缺失目标父树不创建、循环拒绝。
7. 真正保存暂停在 Prepared 阶段：第二次保存排队后更新内存，第二次发布必须使用准入后的新值；暂停期间内存写不被旧读锁阻塞。

第 7 项具有有界等待、失败后的屏障释放和第一任务 join。故障 hooks 仅在私有单元测试模块内，按 canonical fixture target 分区；没有公开测试 API、全局环境变量修改或生产故障开关。

本分工没有运行 Cargo、数据库、Docker、Git 或外部网络。内部回归目前仅为源码完成状态，不能宣称已通过；Root 负责冻结后同源验收。异 uid/gid 的拒绝分支通过静态审阅，未提权制造异 owner fixture；未以真实 FIFO、真实断电或其他文件系统验证此实现。公共 API/HTTP/重启测试由另两名 agent 与 Root 提供。

## Root 最终验收补记

源码 `2f5e2a2267c8499a116e652a4779f111ddbe5aff`；冻结 V1 manifest `c2fc158414333488da098f06050c4daf681477aef50fbdda193b82b111744312`，653/653 源码 Git blob 一致。

macOS ARM64 workspace **1120/0/5**；Linux ARM64 Server 默认并行完整回归 **838/0/5**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；本轮测试阶段均首轮通过，测试期限保持原值。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 0；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文实施时点的未运行说明保留，实际整轮结果以本节为准。

未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。
