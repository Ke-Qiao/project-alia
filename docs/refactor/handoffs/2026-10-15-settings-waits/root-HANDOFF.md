# Settings 等待与关闭 — Root HANDOFF

日期：2026-10-15；基线 `e89d34c`，分支 `codex/architecture-refactor`。用户要求先检查复现、只补缺口，保留政策/部分成功/管理员边界；三个 subagent 已按文件分工实施并独立复审。R03 仍暂停，真实录放音/执行器禁用，不新增依赖。

## 已完成的实现与验证

1. LLM：三个 provider 的六条内部请求路径原先持配置读锁跨网络 await。三个本地 RED 均失败；现在每请求一次完整 snapshot，OpenAI 的协议家族与端点/模型/凭据不跨代。旧请求继续使用旧快照，后续请求读新快照；取消由原 owner 释放 borrowed future/stream。
2. TTS：权重设置原先在 reqwest timeout 之前排队占用错误体额度。RED 两项中 1 失败/1 通过；已将同一单请求期限覆盖 reserve/send/error body。原先先改本地配置、远端失败 warning 的语义保持，未把本轮改成跨模块事务。
3. 关闭：原 pipeline 假设在基线真实运行中已被推翻（1/1通过），因此 API/Runtime 没改。三项最终定向验证证明 shutdown、实际 TCP 断开＋同值重试、privacy 共享锁等待关闭；断开、future Drop 与保存结果分别观察。
4. `providers::` 定向 51/0/0，`settings_shutdown::` 定向 3/0/0，冻结输入 `2b4524993978bcc30c675b4ffc8bd2b2757e8b2250ebdddbcc912932d9fd105e`，无源码漂移。同源 Server 全目标为 868/0/5（50 个 Cargo target，默认并行；431.74 秒），其中 lib 374/0/3、runtime 81/0/0、Registry 取消 2/0/0。定向和子进程重入不重复累计。
5. 整包通过后仅用 rustfmt 将 llm/mod.rs 的 cfg(any(...)) 换行；精确字节转换及其余 668 输入一致已核对。最终 manifest `313671a7cee06b5485bd1f2eb9e895cc23ee347e0ced02eb87ae758e6b7b24ce`，provider 51/0/0 复核通过，9 个改动 Rust 文件格式和 diff 检查通过。无默认特性 all-targets 检查通过，未新增依赖。

## 保留的边界

SQL 确认不确定应重读版本；desired、已提交 SQL、已完成模块效果和远端可能已接受的权重切换不会因取消自动回滚。同值显式重试仍重走 apply/save。同步文件操作不能被 Tokio 抢占；任意扩展内部不可取消工作仍可能拖住整体关闭。没有整体 Settings SLA、无后台代办保存、无新的全局 timeout。

真实 HTTP 证据使用当前 HTTP/1 本地连接；纯 GET/PATCH update gate 排队只有 Registry 局部取消与静态调用顺序证据，没有单独 HTTP 阶段见证。没有扩大到代理/H2/真实模型，也没有重新运行 Embedded、Pi、Linux应用、真实PTY或现场验收。

## 资源、提交和接续

用户已明确允许共享 OrbStack 启动及缓存镜像隔离 PostgreSQL；实际 `orbctl start` 返回 already running，不能据此认定由本次命令启动某共享服务。task DB 为 `alia-settings-waits-20261015-db`，只绑定127.0.0.1；收尾只删除这个准确ID/label的数据库，保留其它服务与运行时。

本包 **10/10** 完成，源码提交 `4e9913414c68b4a78b52c472a04d5a2139785430`；669 个提交 blob 与最终清单全部一致，无额外/缺失输入。task PostgreSQL 已按准确 ID/label 删除并确认不存在；共享运行时与其它服务未停止。证据、清单和恢复检查点已保存。此前 R02 硬件AEC优先、资料缺口、R03暂停及 R05真实整机门槛保持；本轮不能增加R01—R06的2/6完成数。

## 恢复入口与证据口径

从 [本轮恢复检查点](../../resume-checkpoint-settings-waits-2026-10-15.md) 继续；Root 包10/10，model7/7、audio6/6、review7/7。四项真实断言RED（LLM3＋额度1）与两项旧实现本就通过的取消/关闭证据分别保存，不能将整个red阶段说成全失败。

原始日志和命令记录见 [证据索引](evidence-index.json)，去重结果见 [validation-summary](evidence/validation-summary.json)。初次新增 cfg 格式检查失败原件保留；随后精确格式转换与其余668输入一致见 [format-only-transition](evidence/format-only-transition.json)。最终源码的provider复核与无默认检查不重复累计为新增用例；完整868计数来自修改格式前的同逻辑整包，不能声称最后字节版本重新执行过全量。5个ignored仍为真实PTY3＋真实Easy-Turn2，本轮未运行。

下一软件候选是既有V01的有界复现定位或用户指定的部署收尾；不因本包完成自动开启未确认的框架。同步存储不可抢占、可信部署账户/路径和真实模型/关闭行为仍是部署验证项。R02继续等待R1/厂商/输出链资料，R03须用户明确恢复后再开展；R05不能跳过现场出口。
