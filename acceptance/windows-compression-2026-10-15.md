# Windows 1.3.2 压缩实测

所有大小按十进制 MB 计算。本次只生成本地试验产物，未替换 COS 的正式安装包，
也未更新本机或 Company 已安装应用。

| 方案 | 字节数 | MB | 相对正式包节省 |
| --- | ---: | ---: | ---: |
| 已发布 NSIS 安装器 | 75,321,593 | 75.32 | — |
| 对现有 EXE 再做 7z 极致压缩 | 75,305,029 | 75.31 | 0.02% |
| 运行包 ZIP 仅存储，再做 7z 极致压缩 | 51,442,748 | 51.44 | 31.70% |
| 运行包 ZIP 仅存储，NSIS LZMA 固实压缩、64 MiB 字典 | 54,044,179 | 54.04 | 28.25% |

## 参数与产物

使用 [官方 7-Zip 26.04](https://www.7-zip.org/download.html) 的 x64 独立命令行工具，
下载文件已比对官方 GitHub Release API 提供的 SHA-256。

```text
7za a -t7z -mx=9 -m0=LZMA2 -md=64m -ms=on -mmt=10 <output.7z> <input>
```

所有步骤顺序执行，子进程继承 10 个逻辑处理器的 CPU 亲和性限制。

试验目录：`.build/compression-1.3.2/`。

- `benchmark.py`：完整试验驱动。
- `results.json`：各产物大小、SHA-256 和缩小比例。
- `payload-ultra.7z`：51.44 MB 安装内容压缩包，非可直接双击的 EXE 安装器。
- `pixels-agent-bridge-1.3.2-compression-test-setup.exe`：54.04 MB 的 NSIS 试验安装器。
- `extraction-checks.json`：Windows PowerShell 5.1 解压验证结果。

## 校验与限制

未删除 Node.js、Playwright、PDFium、OCR 数据或任何程序。仅把两个内嵌运行包从
ZIP Deflate 改为 ZIP Stored，使外层能够压缩原始字节；EXE 方案另外使用 64 MiB
LZMA 字典。安装器仍使用原有 `Expand-Archive` 和服务安装逻辑。

7z 完整性测试和解压后文件哈希比对通过；两个运行包共 203 个文件经 Windows
PowerShell 5.1 `Expand-Archive` 解压后，与原运行包逐个比对内容一致。

试验 EXE 已完成 NSIS 构建，尚未执行完整覆盖安装验收。正式发布仍为原 1.3.2
安装器，其自动启动更新功能、公开下载哈希、发布签名和更新接口已验证。

## 正式打包方案

用户确认采用 54 MB 左右的 EXE 方案后，已把转换纳入 `build_nsis.py` 的临时暂存
阶段，`setup.nsi` 使用 64 MiB 字典。原始浏览器/文档运行包和跨平台打包输入保持
原校验值，ZIP 成员时间和属性也予以保留。

运行 `python packaging/desktop/test_nsis_compression.py` 通过：203 个实际运行文件
经安装器使用的 Windows PowerShell 解压后，内容全部一致；ZIP 元数据和原始归档
校验值保持不变。正式包按新版本 1.3.3 构建和发布，避免替换已签名的 1.3.2 对象。
