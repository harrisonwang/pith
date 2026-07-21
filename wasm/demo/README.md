# WASM 开发测试页

这个目录不是产品演示，而是仓库内部的 WASM 测试页。它用于快速检查浏览器版能否解析 DOCX、XLSX、PDF、PPTX、HTML、EPUB 和 IPYNB，也会确认浏览器版仍会拒绝过大的文件、损坏的 ZIP、压缩炸弹和旧版 Office 文件。

面向用户的浏览器演示请使用 `examples/cloudflare-pages` 和 `examples/local-corpus-explorer`。

```bash
cd crates/spoor-wasm
npm run build:web
cd ../../wasm/demo
npm run dev
```

打开开发服务器给出的地址，并访问 `/wasm/demo/`。文件只在浏览器内解析，使用 `spoor-core` 的默认限制，每次最多处理 64 MiB。
