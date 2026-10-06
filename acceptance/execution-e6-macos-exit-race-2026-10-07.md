# E6 macOS Git worker 退出竞态

日期：2026-10-07。承接 `0476272` 的及时回收修改；正式 MCP 已恢复，当前目录包含 68 个工具。

## 复现与修复

Mac ARM 使用新父端测试程序和已安装 1.2.28 worker 时，慢凭据程序取消/期限测试通过，
但正常 push 或原结果核对偶发保留为 unconfirmed。
直接核对 worker 返回值后定位到清理阶段 `killpg` 的 `EPERM`，不是 Git push 失败。
定位任务：`66225714-75aa-4f21-95fb-1393a34d8f13`、`1b2b79af-45c6-42fc-bbcb-ca81526e6990`。

原有逻辑只处理先经 try_wait 确认退出的僵尸进程组。直接 terminate 路径未处理此状态，
且有一个可复现的过渡窗口：killpg 已拒绝信号，但 waitid 尚未报告退出。

现在在 macOS 收到 EPERM 后，保留未回收的子进程 PID，最多等待 100ms 核对退出；
**只有 waitid 确认组长已退出、并确认组内没有存活后代时**，才接受为清理完成。
仍运行、仍有存活后代、查询失败或超过期限均保留错误。没有取消权限检查，没有重新执行 Git。
回收错误增加阶段信息，原生 Git 测试直接验证 reconciliation 错误，避免被公开查询的保守状态遮蔽。

## 验证

- 新增 `explicit_terminate_reaps_an_already_exited_group_leader`，独立进程组退出但未 wait/reap，
  直接 terminate 必须回收且允许重复调用。Mac、Linux 通过。
- Mac 正常用户 Git 工作流重复 5 轮均通过，包含原用户只读核对远端提交；
  慢凭据程序取消/期限测试通过；12 项已有 Git 回归通过。
  原生 MCP 任务 `20331383-1ba6-4ef3-b4c0-0e5e26bd675a`，最终 exit 0。
- Windows Executor tests 编译检查通过；Windows 实际清理回归见上一份 E6 报告。
- Mac 生成的用户仓库剩余 0；精确路径进程检查只剩已安装服务 PID 25583，
  检查任务 `e600ea2f-b02c-4f28-b59b-a6ccdeaba7d7`。

测试程序仍是源码父端，调用已安装 worker；没有替换安装文件或 TCC。
本次修复尚未进入正式包，最终交付必须重新构建安装。

## 测试夹具修正

首次运行误填 bundle 内 worker 路径，实际安装位置为
`/Library/Application Support/PixelsAgentBridge/pab-executor`，修正后才进入目标逻辑。
最初新单元测试使用 `/bin/true`，Mac 不存在该路径，改用三平台 Unix 均有的 `/bin/sh -c 'exit 0'`。
这些夹具失败不计作产品回归通过，也没有重复未确认业务操作。
