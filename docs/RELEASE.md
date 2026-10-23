# 安装包发布与在线升级

客户端沿用下面的官网/COS 签名更新流程。管理平台与 Relay 使用独立版本和服务发布入口，
不会写入客户端更新接口。

## 管理平台与 Relay 发布

原生包使用 `python scripts/build.py management --profile release --package` 或
`python scripts/build.py relay --profile release --package` 构建。Docker 使用
`docker-management` 或 `docker-relay` 目标，添加 `--package` 导出按镜像 ID 固定的 tar。
各产品独立计数；同一命令构建某产品的原生包和镜像时只预留一次该产品版本。

必须先提交源码。构建记录绑定源码指纹，归档/镜像清单绑定实际产物；构建期间修改源码
会中止记录。两个版本计数文件属于分配记录，从源码指纹中排除，报告仍绑定准确版本和产物哈希。
版本计数文件应保留并提交，不得删除它们来重新使用已分配的版本。
没有 `.git` 的源码归档仍可自行构建和导出产物，但没有提交来源证明，不能通过正式发布入口。

```sh
# ARTIFACT 为实际生成的 release 归档或镜像 tar；notes.md 是准备公开的更新说明。
python scripts/release_service.py prepare relay --artifact ARTIFACT --notes-file notes.md
# prepare 输出 state.json；其旁边是已冻结的候选产物。
python smoke/run.py run --target relay --profile release --arch x86_64 --version VERSION --artifact FROZEN_ARTIFACT --lab smoke/local/lab.toml
python scripts/release_service.py publish --state STATE_JSON --smoke-report smoke/runs/RUN_ID/report.json --repo OWNER/REPO
```

管理平台将 `relay` 换成 `management`。发布使用 GitHub CLI 和已登录的发布账号，需要私下设置
`PAB_SERVICE_RELEASE_PRIVATE_KEY_FILE`，指向固定的 32 字节 Ed25519 私钥。该文件可以沿用已有
发布密钥，但签名域固定为 `PAB-SERVICE-RELEASE-V1`，与客户端清单隔离。私钥不进入源码、
包、镜像或公开清单。GitHub Release 只上传候选产物及签名 `release.json`，原始测试证据留在本地。

服务标签例如 `relay/vVERSION-linux-x86_64`；镜像导出包添加 `-image` 后缀。
发布显式指定 `--latest=false`，不会替换仓库的 Latest。脚本在任何远程操作前验证对应
产品、版本、架构、提交和产物的完整冒烟报告，再发布；随后重新下载资产核对。
超时或失败保留 `publishing` 状态，使用下面命令核验，不自动重复创建 Release：

```sh
python scripts/release_service.py verify --state STATE_JSON --repo OWNER/REPO
```

若远端仍是草稿、部分上传、指向错误提交或资产不匹配，应人工核对原发布，保留原始失败记录。
不得通过改写本地状态把它标记成功。服务版本由管理员部署，不向客户端推送安装包。

下载者使用另行信任的原始 Ed25519 公钥验证产物，不能使用同一次下载自带的未受信任公钥：

```sh
python scripts/release_service.py verify-file --artifact DOWNLOADED_ARTIFACT --manifest release.json --public-key-file TRUSTED_PUBLIC_KEY
```

镜像 tar 验证后通过 `docker load --input DOWNLOADED_ARTIFACT` 导入，再使用已签名清单的
`image_id` 设置本机部署标签。Compose 必须分别选择管理平台和 Relay 镜像；保留原有配置与卷。
开发阶段统一使用最新版管理平台、Relay 和客户端，发布门禁保留 `INT-01` 整套联调，
不要求新旧版本混用测试。各产品分别计数，发布版本号无需相同。

## 客户端发布

发布前必须完成 [统一冒烟测试](../smoke/README.md)。先在以下发布命令中添加
`--prepare-only` 生成候选包，再对该包执行测试；最后以 `--resume --smoke-report
smoke/runs/<run-id>/report.json` 恢复。上传前会核对必测结果、源码、版本、平台和产物
SHA-256；快速检查、缺失或过期的报告不能代替发布验收。

官网负责版本分配和发布记录，COS 保存安装包。Windows x86_64 与 macOS Apple Silicon 分别递增安装包版本；Rust、npm、Tauri 内部版本保持原值。官网地址默认为 `https://agent.rgaa.vip`。

首次使用时，给构建机配置私有环境变量 `PAB_RELEASE_API_TOKEN`（官网发布令牌）和 `PAB_RELEASE_PRIVATE_KEY_FILE`（32 字节 Ed25519 发布私钥的路径）。密钥、令牌、COS 凭据不能提交到 Git。官网部署机单独保存 COSCLI 配置，构建机不需要 COS 长期密钥。macOS 发布包目前不带 Apple Developer ID 签名或公证；发布清单仍必须使用 Ed25519 签名。

从对应系统的源码目录执行：

```sh
python scripts/release.py windows --notes-zh "本次更新内容" --notes-en "Release notes"
python scripts/release.py macos --notes-zh "本次更新内容" --notes-en "Release notes"
```

每次只选对应系统的一条命令。脚本会预留版本、增量构建完整 EXE/PKG、核对 SHA-256 清单、签名、直传 COS、请求官网校验并发布，最后查询公网更新接口。若中断，原地使用同一命令加 `--resume`；`.build/release-<平台>-<架构>.json` 保存这次发布的 `build_id` 和阶段，成功后自动删除。不要删除未完成状态后再重新发布，否则会浪费版本号。

管理员在 `https://agent.rgaa.vip/admin/releases` 可查看发布历史、未完成上传及最近一次校验错误，并回滚当前指针。手工补发时先在页面预留版本，使用与该版本完全一致的安装包文件名，再在本机运行：

```sh
python scripts/sign_release.py .build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.75-setup.exe
```

将输出的 Base64 签名粘贴到页面，选包并填写中英文更新说明。页面和脚本使用相同的上传与校验 API。回滚仅改变官网当前发布，不强制已升级客户端降级；目标对象必须仍在 COS 且与数据库哈希一致。

客户端启动约 15 秒后检查新版本，之后每 2 小时自动检查一次，也可在“设置 → 关于”手动检查。发现更新后，所有页面顶部显示版本横幅，点击即可进入“关于”页下载。客户端下载后核对平台、版本、来源、文件长度、SHA-256 和 Ed25519 签名，再启动完整系统安装器。支持升级的首版仍需手工安装一次；旧版没有升级入口。升级会短暂中断控制和 MCP 会话，但用户数据与设备身份保留。macOS 图形安装器会要求管理员密码，未签名包可能显示系统提示。安装验证应使用专用测试环境。

签名私钥应离线备份。轮换时先发布一个内置新旧两把公钥的客户端，再把官网切到新签名；待旧客户端过渡后删除旧公钥。直接替换当前单公钥会使旧客户端拒绝新版本。
