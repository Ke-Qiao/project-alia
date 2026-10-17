# PN01 — Server插件管理与本人数据边界

2026-10-17；基线`15604e3`，本分工5/5完成并冻结。仅修改`packages/alia-server/src/api/plugins.rs`和`packages/alia-server/src/plugins/storage.rs`；测试由fixture分工编写、Root统一执行。

## Checklist

- [x] 核对全部管理handler、存储查询和调用者；复用Settings末尾身份复验及Context事务`FOR SHARE`模式，不重构全局AuthUser。
- [x] 读取Root原实现RED证据：2项均按预期失败。普通用户export返回200并暴露另一owner；purge返回200，完整记录数9→1，目标插件8条跨scope记录被删除。
- [x] 修复四个管理入口和本人数据入口，保留CAS／TTL／retention和Execution批准。
- [x] 限定两文件rustfmt及`git diff --check`通过，源码候选已冻结。
- [x] 独立审阅及Root必要回归与资源取证收口：生产静审GO，HTTP 7项与既有storage 4项通过，任务PG回收前schema／client均0。

## 最终实现

既有`GET /plugins`、`GET /plugins/{name}/storage`、`GET /plugins/{name}/storage/export`及`DELETE /plugins/{name}/storage`仅供当前active管理员。初始AuthUser角色快拒普通用户；所有敏感读在最后一次存储等待后，以2秒有界SQL重新核对active/admin才构造响应。

新增`GET/DELETE /plugins/{name}/storage/me`与`GET /plugins/{name}/storage/me/export`。owner唯一来自服务端`AuthUser.id`，存储SQL固定`scope='user' AND owner_id=actor`，不猜测channel／conversation归属。GET拒绝额外query；DELETE只接受可选collection，`owner_id`／scope等未知字段被拒绝。本人响应前复验active，不要求admin。

两个DELETE共用原存储删除查询的事务入口。先在事务内执行DELETE，经过实际表／行锁等待后，再以同一事务`SELECT ... users ... FOR SHARE`核对当前actor；授权失败显式rollback，成功持有actor锁直到commit。这样等待期间已提交的降权／失活不能被初始快照授权；最终复验后的账户修改也不能先于本次删除提交。若rollback本身失败，返回数据库错误，不将删除报告为成功。

查询helper增加可选actor过滤并支持事务连接，原受信编译插件接口及非HTTP内部使用不变。usage继续包含过期记录及其计数；export继续排除TTL已过期记录；purge继续可删除过期记录。原compare-and-put、collection声明、单条上限、卸载retain/delete/export_required规则未修改。没有增加第二套授权账本或用户自选scope入口。

## 验证与边界

原件：`run/validation-20261017-pn01/red.log`，2项RED均失败于预期403，实际200；未把RED失败当最终修复结果。已独立核对`green-http.log`：7 passed／0 failed／0 ignored／85 filtered，测试体3.32秒。普通用户全局export／purge均403且9条原记录不变；管理员管理正例与本人scope／参数隔离通过。三个等待用例均有真实SQL等待见证，AdminExport、AdminPurge、PersonalPurge失权后全部403、`changed=false`且未暴露另一owner。独立生产审查GO。

已独立核对`green-storage.log`：既有storage 4 passed／0 failed／0 ignored／0 filtered，测试体0.07秒；真实PostgreSQL round-trip、CAS、TTL与retention用例通过，原件没有skip。Root确认使用本轮真实任务PG；`resources-before-stop.json`记录fixture schema 0、其他database client 0。Root随后完成任务容器`docker stop`，`--rm`回收后列表为空。本轮只做必要HTTP与storage回归，不声称新跑完整workspace；源码保持上述GREEN字节，交Root统一提交。

本轮只落实已确认D01的HTTP管理与本人数据边界，不交付隔离插件broker、动态安装／升级、第三方隔离后端、总量quota或channel/conversation成员授权。全局AuthUser／Settings未修改；默认shadow、Execution批准、硬件范围及R03暂停保持。真实子进程／硬件与新平台均未启用。
