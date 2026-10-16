# CompanionInvitation 历史只读引用 — Root

2026-10-16 批次，2026-10-17 凌晨继续收口；基线 `95cd077` / source `9e3a934`，分支 `codex/architecture-refactor`，工作区起始干净。本轮10/10完成；源码提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`，677项输入逐blob一致。

## 契约与实际改动

用户确认已提交的邀请在输出期限到期、服务重建/重启或主动模式改回shadow后仍可作为历史事实读取；来源删除、隐私撤回和当前对话读取政策继续生效。历史读取不恢复播放、批准或主动表达权；已提交文本不是用户已听到的证据。

现有materialize只允许Generation父context，真实已提交邀请因purpose过滤被遗漏。实现新增独立 `CompanionHistory` marker；Message/Response/Context正文、摘要、owner/session/channel及不可变admission身份核对后纳入普通Generation。当前用户source/request不被改成邀请身份。仅排除CompanionAdmission，保留完整来源闭包和来源自己的最早期限；不继承原邀请输出TTL。原invitation purpose仍只有两种输出用途，通用parent validator未放宽。

Memory根绑定保存marker与完整闭包，支持既有synthetic Memory根验证。携带该历史来源的Appraisal不能再次成为Companion来源，包括间接Memory/cache来源；这是防止自我强化的保守限制，不是把历史内容转为主动输出许可。路由/技能/主模型等已有调用链共用generation.history_messages，无需另建注入入口。

生产触及7文件；新测试1文件。另对 `tests/server_runtime/delivery.rs` 的1条既有import排序做格式化，已先核对该文件与HEAD逐字节相等，无行为变化；Cargo manifests/lock与依赖未改。时间身份沿用微秒约定，是规范一致性调整，当前路径两侧均来自PG，并未复现纳秒故障。

## 分工与证据

三个subagent：fixture负责契约/7文件生产，flow负责RED/7项新增回归，review负责独立权限/来源/锁序与证据审查。Root统一Cargo、隔离PG、归档和提交。所有源改动在RED之后；原45秒fixture期限、默认libtest并发与CASE许可保持。

- RED：0/1/0/377filtered；唯一目标失败为已提交邀请被普通Generation遗漏，0.47秒测试体/66.77秒命令。同库结束后0schema/0client。677项输入 `dfb4b737298d557008c6e357bd9d81fbf7faf96162232655f071d2bf21c087e1`；旧源码与失败日志单独保留。
- 一次定向GREEN：13/0/0/371filtered，包含原6项与新7项；5.63秒测试体/49.54秒命令，drift为空。同库0/0。677输入 `b6b481d699aa752e7a98aad2a50ea5e9f966b902d73e2ee48a4a0f0751ff6e5d`。
- 最终冻结：677项 `4713f1a8f475c8524ca8c8dec270fbec441a984eea913bc8d49f98482b785ac2`，与GREEN逐项相同。fmt/diff检查通过。一次完整workspace **1277/0/5/0filtered**，72目标（67Running+5Doc），555.32秒；Server library381/0/3、server_runtime85/0/0、duplex31/0/0。5项既有ignored为PTY3项和真实Easy-Turn2项，无新增ignored。GREEN与最终二进制分别保留，最终Server库SHA-256 `9f1c2c3585c4124a9acf1198ffbb9e8e46ef3cd2685045ad0d9e4473c84dbe25`。定向与全量不相加。

## 验收边界和恢复

新测试覆盖未提交/owner/session/channel、输出到期但根TTL保留、退休privategrant清除、ContextService重建与shadow、八类来源/内容/政策/隐私/关系撤回、二级历史marker缺失/伪造/混入admission、真实Memory写入与cache合并、防Companion再次取源。服务对象重建不等于当前二进制的进程重启验收；所有根类型故障注入也不是穷尽证明。实际模型专项进程未启用，既有pipeline回归由完整workspace执行。

裸删Message/Response之后，来源校验会拒绝新引用及旧派生授权；管理API和会话展示不因此自动物理删除全部派生数据。本轮不新增通用擦除框架；真实privacy erase和relationship clear继续沿用现有事务擦除链。详见[契约](../../companion-invitation-history.md)。

没有Linux/Pi、真实模型/音频/执行器验收，无依赖/平台扩大。默认shadow、当前政策、权限和执行批准保持；R03暂停，历史V01根因open，六轮仍2/6。完整测试结束后约0.258秒，同库资源为0schema/0other clients；已按exact任务容器ID/label回收容器和卷，保留共享astrbot/OrbStack。源码提交与冻结677项逐blob一致，34份原件共656064字节逐字节归档于evidence，独立索引在evidence-index.json；恢复材料随文档提交。归档脚本保持原run目录布局，不能直接在evidence目录当通用工具执行。

后续软件主线是有界的软件发布验收：当前源码的Linux构建/测试/运行矩阵、全noop staging进程启停重启、待确认端点后的真实模型链及冻结发布身份。历史V01继续有界跟踪，不用本轮通过声明根因消失。完成这些软件出口后，按R02资料/声学门槛和用户恢复R03的决定推进R05。
