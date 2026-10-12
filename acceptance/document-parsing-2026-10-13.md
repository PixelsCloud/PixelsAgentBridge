# AnyDoc + 离线 OCR 验收记录

日期：2026-10-13。安装包版本：V1.2.93。目标是让 Agent 在选定设备上解析文档，并通过 MCP 按位置读取、搜索和预览页面；解析在可终止的独立工作进程内进行。

## 已安装设备

| 平台 | 测试设备 | 安装包与结果 |
| --- | --- | --- |
| Windows x86_64 | Company，设备码 559038160 | 74,264,002 字节；SHA-256 `715652ab6068ca8328d3de0ce3ea666d3ad694afc5ea3fe5b0ebe93ad46444d6`；静默覆盖安装退出码 0。 |
| macOS arm64 | Mac，设备码 419438177 | 85,931,193 字节；SHA-256 `faa8684e6e982a42ee9ff9316c6e28a77da55bbaf8632b981ce4a72a5b10b2a8`；系统安装器退出码 0。PKG 没有 Apple Developer ID 签名或公证；程序与 PDFium 使用既有本机证书签名。 |
| Linux x86_64 | Debian 构建容器 | 46,600,935 字节；SHA-256 `0cab13426c939d56307ea08fb70765f966281220c75227c858966690002f6808`。包内二进制与构建产物哈希一致，离线运行时齐全。 |

两个已安装设备都通过了正式 `pab-mcp → Bridge → Executor` 链路：PDF 混合原生文字与中英文 OCR 页、DOCX 表格、XLSX 单元格与公式、PPTX 幻灯片与讲者备注、PNG OCR，及 PDF 单页 MCP 图片预览。测试还覆盖 `pab_document_open/read/search/render_page/close` 全部五个工具和 `execution=user` 的读取。Company 的 25 页 OCR 任务中，`pab_system_info` 在 0.1 秒内响应；取消任务进入终态，工作进程没有残留。Mac 安装后的工作进程与 Linux 容器工作进程均通过 10 组样本，包括加密/损坏文件及压缩炸弹。Linux 尚未在独立实体机上安装，验收范围是 headless 工作进程与包内容。

## 本地与打包检查

- `pab-document` 13 项单元测试及 `pab-protocol` 56 项单元测试通过；Executor 的幻灯片备注定向测试通过。
- `scripts/test_build_version.py` 15 项、`packaging/desktop/test_packaging.py` 和 `scripts/test_build_inputs.py` 通过。
- Windows 与 Mac 安装包包含 `pab-document-worker`、PDFium 和简体中文/英文 Tesseract 数据；无目标机运行时下载。Mac 本地证书没有 Apple Team ID，因而仅给短命文档工作进程设置 PDFium 所需的库验证例外；主程序与 Executor 没有获得该例外。

可复现的工作进程样本位于 `acceptance/fixtures/document`，正式工具链脚本是 `acceptance/document_mcp_live.py`。运行脚本需要先通过正常 Pixels MCP 环境连接目标设备；测试不会记录账号、密码或文档正文。

## 当前边界

解析输入上限 64 MiB、PDF 最多 500 页、工作表累计最多 25 万非空单元格；单设备同一时间只运行一个解析进程。解析快照在 Executor 内存中最多保留 1 小时，服务重启后失效。单次读取最多 14 KiB，可按 `next_cursor` 续读。OCR 固定为简体中文和英文；旧版 DOC/XLS/PPT、加密文件解锁、编辑原件与远程 URL 抓取不在此次支持范围。
