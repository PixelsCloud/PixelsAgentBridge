# E6 Windows 应用辅助进程超时与原操作保留

日期：2026-10-07，源码基线 `1526f4e`。本轮不分配版本、不替换安装程序。

## 改动

从 Windows 应用专用 helper 提取原有 20 秒调用期限到 `application_deadline.rs`。
生产入口和测试使用同一函数：阻塞线程调用超时后退出当前进程，成功返回结果，线程 panic 返回错误。
期限与退出策略不变；没有给发布程序加入故障注入参数。

## 原生进程测试

Windows 本机运行 `cargo test --locked -p pab-executor --test application_deadline`，
成功返回与 panic 错误返回检查通过；三个专用进程/耗时用例默认 ignored。
再显式运行 `blocked_native_call_retires_only_helper_and_keeps_application_alive`，通过，20.53 秒：

- 独立 helper 测试进程启动仅属于测试的子进程，等待它就绪。
- 实际阻塞线程进入 Win32 `Sleep(INFINITE)`，模拟无法取消的原生调用。
- 生产期限函数让 helper 以退出码 1 结束；父测试设置 35 秒故障界限并核对没有提前返回。
- helper 退出后，子进程仍持续写入新的心跳；由专用 stop 文件正常关闭。
- 两个进程均结束，临时目录自动清除，进程清单无 `application_deadline*` 遗留。

本用例验证实际进程退出和子进程存活，不把模拟阻塞称为真实第三方 Shell 扩展兼容性验收。
未使用当前用户的真实应用、文件或窗口，不发送键鼠事件。

## IPC 与持久记录

原应用 IPC 用例新增数据库重开和新 UI 连接后的断言：
已派发但 helper 断开的启动请求仍为 `unconfirmed`，保留原用户身份，重复请求不发给替换 helper。
Windows、Mac 各 4 项应用 IPC 测试全部通过（Windows 1.15 秒、Mac 1.05 秒）。
另覆盖原窗口通道隔离、重复请求持久化和错误身份/错误结果类型拒绝。

双桌面 `cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib` 通过。
Mac 通过正式 Pixels 用户命令执行，任务 `03d38991-9c96-4d09-aa82-535be875d33a`，退出码 0。
Mac 编译仅有既有 enigo unnecessary unsafe 与 agent_integrations unused_mut 警告。
Linux 不运行 Windows 应用 helper，测试入口用 `cfg(windows)` 明确隔离，未增加 Linux 桌面能力。

## 未覆盖

进程退出测试和 IPC 持久记录测试是两层证据，尚不是已安装 supervisor、WTS 原始令牌、
真实 Shell 调用、正式 MCP 请求合在一起的端到端故障注入。
安装后多活动会话、普通用户/split-token 管理员、应用存活及辅助进程自动补充仍待验收。
新的完整包还须包含此前 Git 工作进程清理和终端关闭修复。
