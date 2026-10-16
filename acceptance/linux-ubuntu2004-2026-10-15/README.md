# Ubuntu 20.04 兼容与官网一行安装验收

日期：2026-10-15。Linux x86_64 stable **V1.0.2** 已签名发布到 COS，官网一行安装入口与下载区均已更新。

```bash
curl -fsSL https://agent.rgaa.vip/install.sh | bash
```

## 实现

- 使用 Rust 1.95 / Debian 11 的 glibc 2.31 编译环境，固定到基础镜像所用的 Debian 软件源快照；与旧编译基线分离缓存。Cargo 8、CMake 2，总并发不超过 10。
- 使用发行版 `python3-cryptography` 的 Ed25519 验签接口，按需自动安装该依赖；保留签名、大小、SHA-256 和归档验证，不要求用户升级 OpenSSL 或替换 glibc。
- 本机 WSL 实际为 **Ubuntu 20.04.2**，不是 22.04。验收前后仍为 glibc 2.31、OpenSSL 1.1.1f、Python 3.8；签名库 cryptography 2.8。
- 官网 Linux 下载卡直接显示安装命令、取设备码与密码命令及复制按钮，并说明支持 Ubuntu 20.04+、Debian 12+、x86_64 / glibc 2.31+。

## 产物

- `pixels-agent-bridge-linux-x86_64-release-1.0.2.tar.gz`，47,422,010 字节。
- SHA-256：`15965d7b3a305d9ed97af6bd805eaa2e689fd9a30c93033d9b11061c0a2aef71`。
- 干净源码构建提交：`eb1685aa636871a6a43ddf68ea883c67d7d0c1db`。
- ELF 最高引用：三个程序的 GLIBC 均为 2.30；Executor / 文档 Worker 的 GLIBCXX 为 3.4.26，MCP 无直接 GLIBCXX 引用。声明的支持下限保持已实测的 glibc 2.31。

## 已通过的验证

| 项目 | 结果 |
| --- | --- |
| Ubuntu 20.04 签名与边界 | 本机 Python 3.8 / cryptography 2.8 的 5 项引导测试通过，包含真实签名、篡改、危险归档、下载损坏及低于 glibc 2.31 时在下载/安装前停止。 |
| 打包 | 打包允许列表、权限、版本及产物来源校验通过。 |
| 本机 WSL | 候选包先经哈希、ELF 和运行依赖预检后安装；正式发布后再执行官网命令重装，设备码及密码保持不变。systemd 服务重启后远程连接与使用正常。 |
| 在线安装矩阵 | WSL Ubuntu 20.04.2，以及新建的 Ubuntu 20.04、22.04、24.04 和 Debian 12 容器，全部直接从官网入口下载 COS 正式包并完成验签、安装及设备注册。 |
| 服务模式 | WSL、Debian 使用真实 systemd；三个 Ubuntu 容器使用 `--no-service` 加外部进程启动。 |
| 真实远程使用 | 五台环境均用设备码/密码通过 Windows MCP 认证，验证命令、中文文件写入/读取、交互终端、日志来源查询、PDF 解析、端口转发读取 HTTP 内容。新设备先等待其已保存设备缓存可见，再查询文档异步结果。 |
| PDF/OCR | 本机 WSL 安装后及正式在线重装后，两次运行既有 10 个文档 Worker 样例均通过，覆盖中文/英文扫描 OCR、原生及混合 PDF、Word、表格、演示文档、加密/损坏和资源限制。 |
| 官网实际页面 | 真 Chrome 打开线上主页：两条命令的复制内容正确，Linux 卡不再以下载按钮为主入口；手机无横向溢出，中英文与暗色模式通过，页面运行错误 0。 |
| 发布 | Linux 查询返回 V1.0.2，COS 文件存在且大小匹配；当前版本查询无新版本。Windows/macOS 原发布版本保持不变。 |

## 清理及保留

四个新建容器已删除；测试用 HTTP 帮助进程、临时密码文件及本机凭据缓存已清理。连接验证会自动产生账号设备条目，已按确切 ID 删除本轮五个及先前 V1.0.1 的两个测试条目，并查询确认删除；未删除其他设备。

本机 WSL 的 V1.0.2 安装及设备数据保留，服务仍运行，设备码为 **779303687**。在 WSL 内查看连接密码：

```bash
sudo /opt/pixels-agent-bridge/run-executor.sh show-access
```

没有更新 Windows 已安装的 Pixels，没有终止/重启 WSL 实例，也没有升级其系统 glibc/OpenSSL。没有进行整机重启或长时间压力测试。

去敏回执见 [results.json](results.json)，线上页面见 [桌面截图](linux-install-desktop.png) / [手机截图](linux-install-mobile.png)。私有测试脚本、日志位于 `.build/linux-compat-20261015/`；账号令牌未写入报告。
