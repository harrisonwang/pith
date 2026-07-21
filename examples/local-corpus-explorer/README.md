# 浏览器本地文档库

这个示例在浏览器中批量解析本地文档，支持全文搜索，还可以导出 JSONL 和文件清单。文件不会上传，单个文件失败也不会中断其他文件。

这个示例可以：

- 混合处理 PDF、DOCX、PPTX、XLSX、HTML、EPUB 和 IPYNB；
- 列出警告和解析失败的文件；
- 在多个文件中搜索全文；
- 通过 `spoor://` 链接提取图片；
- 导出分段后的 JSONL 和文件清单。

## 运行

```bash
cd examples/local-corpus-explorer
npm install
npm run dev
```

检查与部署：

```bash
npm run check
npm run deploy
```

在线演示：[spoor-corpus-demo.pages.dev](https://spoor-corpus-demo.pages.dev)

## 注意

- 单个文件最多 16 MiB。示例没有限制文件数量、所有文件的总大小和总输出，也没有取消按钮。
- CSV/XLSX 仍默认每张表只取前 100 条数据行。
- spoor 不识别图片内容；需要时应另交给视觉模型。
- JSONL 只包含解析和分段结果，不包含向量化和检索服务。
