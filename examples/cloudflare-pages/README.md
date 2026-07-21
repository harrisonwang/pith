# Cloudflare Pages 示例

同一个页面支持两种解析方式：

- **浏览器本地**：文件不上传，直接调用 WASM；
- **Pages Functions**：把文件发送到 Cloudflare，并在 Pages Functions 中运行同一个 WASM 包。

页面会显示警告，并列出 DOCX、PPTX 和 PDF 中可以提取的图片或图表链接。提取始终在浏览器本地完成；spoor 只返回文件内容，不识别图片里写了什么。

## 本地运行

```bash
cd examples/cloudflare-pages
npm install

# 只启动静态前端，使用浏览器本地解析
npm run dev

# 同时启动前端和 Pages Functions
npm run dev:pages
```

检查与部署：

```bash
npm run check
npx wrangler login
npm run deploy
```

在线演示：[spoor-pages-demo.pages.dev](https://spoor-pages-demo.pages.dev)

## 注意

- 每个文件最大 16 MiB。
- 使用 Pages Functions 时会上传原文件；在浏览器本地解析时不会上传。
- Pages Functions 版本没有身份认证、限流和用户数据隔离，不能直接对外提供服务。
