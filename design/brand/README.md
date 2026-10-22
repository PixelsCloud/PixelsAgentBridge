# Pixels 透明品牌标志

沿用现有青绿色三节点连接标志（`#007276`），移除白色圆角底板与外围描边。节点、连线和比例保持原有设计，重新平衡画布留白。标志内部及画布背景均透明，没有投影或容器底色。

- `pixels-mark.svg`：矢量源文件，用于网页。
- `pixels-mark-512.png` / `pixels-mark-1024.png`：带 Alpha 通道的透明 PNG，用于 Android 应用内和其他位图场景。
- 网页副本：`apps/web/src/brand.svg`，登录页、侧栏与网页图标共用。
- Android 副本：`apps/android/app/src/main/res/drawable-nodpi/brand.png`，应用内保持原色、不染色。

安装 Web 开发依赖并提供 Chrome 后，运行 `node design/brand/export.mjs` 可从 SVG 重新导出所有副本；可用 `PAB_BRAND_BROWSER=msedge` 切换渲染浏览器。不改动桌面安装包及已安装客户端。

网页品牌文字 `Pixels` 使用官网相同的 10 Pixel Bold 字体；`Agent Bridge` 使用原有界面字体的 700 字重。字体来源及许可证见 [字体说明](../../apps/web/src/assets/fonts/README.md)。
