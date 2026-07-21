# Tauri 2 桌面示例

把 `spoor-core` 直接编译进 Tauri 应用，在本机解析文档，不启动其他进程。

```bash
cd examples/tauri-desktop
npm install
npm run check
npm run tauri:dev
```

构建桌面应用：

```bash
npm run tauri:build
```

示例通过 `Array.from(Uint8Array)` 把文件内容传给 Rust，这会额外复制一份数据，因此只适合演示。处理大文件时，应由 Rust 读取文件，或减少前后端之间传递的数据。

只看 `spoor-core` 的基本用法，请参考 [Rust 示例](../rust-core-embed/)。
