# Electron 桌面示例

这个示例在 Electron 主进程中使用 `@harrisonwang/spoor` 解析本地文档。渲染进程开启 `contextIsolation`、关闭 `nodeIntegration`，不能直接使用 Node.js，只能通过 preload 暴露的方法把文件交给主进程。

```bash
cd examples/electron-desktop
npm install
npm run check
npm start
```

构建未签名应用：

```bash
npm run package
```

生成的文件位于 `dist/`。当前 Node.js 包支持 macOS arm64/x64、Linux x64 GNU 和 Windows x64 MSVC。

示例在主进程中解析文件，只用于说明用法。正式应用如果需要强制超时或隔离崩溃，应改用 Electron Utility Process 或独立进程。
