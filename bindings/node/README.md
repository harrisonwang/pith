# @harrisonwang/spoor

在 Node.js 和 Electron 应用中解析文件内容，提示可能遗漏的部分，并查找模型引用。它和 CLI 共用同一套 Rust 解析代码，需要 Node.js 18 或更高版本。

```bash
npm install @harrisonwang/spoor
```

```js
const fs = require('node:fs');
const { locateQuote, parseBytes } = require('@harrisonwang/spoor');

const result = parseBytes(fs.readFileSync('report.pdf'), {
  sourceName: 'report.pdf',
  pages: [1, 3],
  provenance: 'block',
});

for (const warning of result.warnings) {
  console.warn(warning.code, warning.location);
}

const markdown = result.content.value.markdown;
const found = locateQuote(
  markdown,
  '需要查找的引文',
  result.provenance?.spans,
);
```

表格可以使用 `sheet`、`rows`、`columns`、`limit` 和 `offset` 筛选；PDF/PPTX 使用 `pages`。`extractMedia(data, uri, options)` 可以通过解析结果中的 `spoor://` 链接提取对应图片或文件。

完整参数、返回值、警告和错误码见[接口参考](../../docs/API_REFERENCE.md)。
