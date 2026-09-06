# Skill Man Logo · 模块 S

两段相向的圆角模块组成抽象 S，呼应 Skill 与技能的组合管理。延续产品的蓝色方向，使用纯色、无阴影、无渐变设计。

## 资源

- `app-icon.svg`：1024 × 1024 矢量母版，蓝色 #2474DB，白色标志，画布四周透明。
- `app-icon-{16,32,64,128,256,512,1024}.png`：RGBA PNG 导出。
- `mark.svg`：透明背景的单色独立标志，24 × 24。
- `menubar-template.svg`：18 × 18 菜单栏专用母版；2 px 线宽，中心净间距 2 px。
- `menubar-template-18.png` / `menubar-template-36.png`：1x / 2x 黑色加透明通道模板图标。
- `menubar-template-18.rgba`：18 × 18 × 4 字节原始 RGBA，可供当前 Tauri tray 图像加载方式使用。
- `preview.png` / `preview.svg`：设计展示及实际尺寸预览。菜单栏为模拟场景。

## 接入说明

本套设计已接入 `src-tauri/icons/`，打包配置引用 `icon.icns` 和 `icon.png`，菜单栏继续使用 18 × 18 的 `tray.rgba`。菜单栏应启用 template 模式，由 macOS 根据菜单栏背景处理显示颜色；不要把蓝色底板用于菜单栏。ICNS 使用 `npm run tauri -- icon docs/design/skill-man-logo/app-icon.svg --output <临时输出目录>` 生成，再将 `icon.icns` 复制至 `src-tauri/icons/`。更新设计后须同步矢量母版、PNG、ICNS 和菜单栏 RGBA。

已检查设计展示的渲染结果和深浅背景下的菜单栏图形，尚未执行原生菜单栏运行验收。

## 重新生成

需要 Node.js 和 sharp。执行 `node generate.cjs`；若 sharp 位于独立运行时，可通过 `SHARP_MODULE` 环境变量提供其绝对路径。
