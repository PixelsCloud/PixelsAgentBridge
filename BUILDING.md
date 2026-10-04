# 统一构建与版本号

需要 Python 3.11+、项目固定版本的 Rust，以及对应目标所需的 Node/npm、NSIS 或 Docker。先在需要构建的 `apps/desktop`、`apps/web` 目录执行 `npm ci`。

## 版本规则

`build-version.json` 是版本来源，随代码提交。首次整体编译使用 `1.2.0`，随后每次构建 patch 加1；`1.2.99 → 1.3.0`，`1.99.99 → 2.0.0`。minor、patch 范围均为0–99。

统一入口在启动子组件前分配一次版本，并同步自有 Rust crate、两个 Cargo.lock、前端 package/package-lock、Tauri 配置。第三方依赖及协议 schema 版本不变。一个命令内的所有目标使用同一版本，打包不再次递增。构建开始后失败或中断也占用该版本，重新构建递增；同一工作目录禁止两个整体构建并发，以免编译中途版本改变。锁随进程退出自动释放。

版本文件、同步后的清单和锁文件应一起提交。此计数在当前 Git 工作目录中分配，不是跨克隆的中央发号服务；正式构建使用同一个发布工作目录。

## 常用命令（仓库根目录）

```powershell
# Windows Desktop、Executor、MCP；同时生成 ZIP 和原名 EXE 安装包
python scripts/build.py desktop --package

# Server、Relay 和 Web；生成当前宿主平台归档
python scripts/build.py server --package

# 一次整体编译，全部组件共用一个版本号
python scripts/build.py desktop server --package

# 只编译前端，每次命令同样递增一次
python scripts/build.py web
python scripts/build.py desktop-web

# Windows 宿主通过 Docker 构建 Linux 客户端
python scripts/build.py linux --package

# 正式 Docker 镜像，显式使用 release；默认标签为产品版本号
python scripts/build.py docker --profile release
```

默认使用 debug；只有明确指定 `--profile release` 才使用 release。Docker 服务端镜像固定使用 release，因此该目标要求明确指定。`--image` 可指定 Docker 标签。不要用 `docker compose build` 代替统一入口；构建后将 Compose 的镜像变量指向生成的标签，再使用 `up -d --no-build`。

`apps/desktop` 和 `apps/web` 的 `npm run build` 已接入各自的前端构建入口；`apps/desktop` 的 `npm run build:desktop` 编译完整 Windows 客户端。Tauri 内部调用 `build:assets`，避免再次递增。

`cargo build`、直接调用 Tauri/Docker、`npm run build:assets` 是底层组件命令，只使用当前已同步版本，不负责分配产品版本。开发热更新、`cargo check/test/clippy/fmt`、前端类型检查和测试不递增。正式产物必须使用上面的统一入口。

## 打包与版本校验

`.build/builds/` 记录成功构建的版本与产物 SHA-256。现有 `packaging/*/build*.py` 仍可单独重打包已构建产物，保持该产物的版本；无构建记录、组件被替换或混入另一批前端时拒绝打包，不给旧二进制贴新版本。

Windows 安装包文件名保持 `pixels-agent-bridge-windows-x86_64-debug-setup.exe` 等原有名字。版本写入 EXE 文件属性、Windows 已安装应用信息及 SHA-256 清单；Desktop 关于、MCP 握手、设备和 Relay 版本上报、Web 服务版本使用对应编译版本。构建命令不执行安装或部署。

## 验证

```powershell
python -m unittest discover -s scripts -p test_build_version.py -v
```

覆盖首次编译、连续递增、两级进位、非法版本、清单/锁文件同步、第三方版本不变、失败重试、跨进程互斥、多目标只分配一次，以及打包版本和产物哈希匹配。测试在临时目录运行，不消耗正式版本号。

2026-10-04 验证：9项自动化测试通过；`python scripts/build.py desktop server --package` 完成首次 `1.2.0` 实际构建，版本计数为1。Windows 客户端 ZIP/NSIS、Server/Relay/Web 归档均生成成功，Desktop 与安装器的 ProductVersion/FileVersion 均为 `1.2.0`。17个自有 Rust 包版本一致，两份 Cargo.lock 的第三方条目未变。构建日志 `.build/versioned-build.log`。本轮未执行 Linux/Docker 镜像实际构建。
