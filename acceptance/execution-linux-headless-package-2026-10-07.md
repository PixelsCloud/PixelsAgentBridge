# Linux 无界面打包与安装测试

日期：2026-10-07。源码版本 1.2.26；本轮为组件/安装回归，不分配正式产品版本。

## 改动

- Linux 构建仅编译 Executor/MCP，不再构建前端或 Tauri，不要求 npm。
- Docker 编译依赖去掉 WebKit、GTK/AppIndicator、PipeWire、Xdo 等 GUI 开发包。
- Linux tar 只包含两个程序、服务/安装脚本和安装说明；不打入过期的 Desktop 输出。
- 默认以 root 安装 systemd 服务；明确指定 `--no-service` 可用于无 systemd 容器/外部进程监督器。
- 不再创建用户 Desktop 本地访问密钥及图形自启动项。升级清除旧产品 GUI 入口，保留机器和用户数据。
- 安装/卸载按 `/proc/PID/exe` 的精确安装路径停止程序；先 TERM 等待最多 30 秒，再 KILL 等待最多 10 秒。
  systemd 服务自身使用 60 秒停止期限及 control-group 回收。
- `run-executor.sh` 转发参数，已有 `show-access` 可显示设备码及密码，安装器不输出密码。

## 已通过

1. 精简镜像 `pab-linux-headless-build-deps:bookworm` 中实际编译两个 Rust 程序：成功（2m12s）。
   未使用前端/Tauri；ldd 只有 glibc、libm、libgcc_s 和 loader。
2. `python -m unittest discover -s scripts -p test_build_version.py`：15 项通过，包含缺失 npm/node 时 Linux 构建仍可分配一次版本并只记录两个程序。
3. `test_linux_package.py`：真实运行打包脚本，验证完整归档白名单、权限、LF、版本和二进制篡改拒绝。
   二进制为隔离测试夹具，不冒充正式产物。
4. `test_linux_install.sh`：Debian 无显示/无 systemd 容器，使用真实源码程序验证
   手动安装、缺失 systemd 默认拒绝、无效 URL 拒绝、配置中的 shell 文本不会执行、CLI 参数转发、
   无 GUI 依赖、升级保留数据、只停止安装目录内程序、卸载保留数据。
   增加忽略 TERM 的隔离产品进程，确认等待后 KILL 回收；安装外同名程序继续运行。
5. `test_linux_systemd_install.sh`：Docker Desktop 中真实 systemd PID 1、独立 cgroup namespace，
   实际安装程序、enable/start/stop、重装、切换 manual 后禁用/移除服务、再安装和卸载全部通过。
   该夹具使用 `wss://127.0.0.1:1/control`，不连接外部服务；服务 active 仅证明进程/管理器生命周期，
   不代表控制连接正常。测试数据标记得到保留。

测试镜像：基于上述 headless 镜像，增加 `systemd dbus python3`，以 `/lib/systemd/systemd` 启动。
systemd 容器使用 `--privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock --tmpfs /tmp`；
源码只读挂载，不挂载宿主 cgroup，不修改宿主服务。专用容器测试后已删除。
手动测试容器使用 `--rm --init`，退出后删除；测试没有访问生产设备数据或外部服务器。

## 尚未验收

正式版本 tar、真实服务器注册和设备码/密码显示、当前 AI 宿主远程连接、用户工具两轮工作流、
升级后真实设备身份/凭据保持属于 E7/E8，不能用本报告的生命周期和标记文件测试替代。
Windows/macOS 安装路径保持原逻辑，本轮未重新打包或安装这两个平台。
