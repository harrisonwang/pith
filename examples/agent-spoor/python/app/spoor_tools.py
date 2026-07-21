"""spoor 能力的共享实现：native 工具与 MCP server 都调它，保证两条路结果一致。
用 pyspoor（同进程原生扩展）。"""

from __future__ import annotations

import json
import os
import re

import spoor

from .validate import opt_num, opt_str, pair, safe_resolve, str_arr

MAX_BODY_BYTES = 96 * 1024


def read_document(
    rel_path: str,
    *,
    pages=None,
    sheet=None,
    rows=None,
    columns=None,
    limit=None,
    offset=None,
    provenance=None,
) -> str:
    """读取文档 → LLM 可直接消费的文本（含 warnings 与元信息）。"""
    abs_path = safe_resolve(rel_path)
    with open(abs_path, "rb") as f:
        data = f.read()
    result = spoor.parse_bytes(
        data,
        source_name=rel_path,
        pages=pages,
        sheet=sheet,
        rows=rows,
        columns=columns,
        limit=limit,
        offset=offset,
        provenance=provenance,
    )
    return _format(rel_path, result)


def _format(rel_path: str, result) -> str:
    if result.content.kind == "document":
        body = result.content.value.markdown
    else:
        body = json.dumps(list(result.content.value.tables), ensure_ascii=False, indent=2)

    raw = body.encode("utf-8")
    truncated = len(raw) > MAX_BODY_BYTES
    if truncated:
        body = raw[:MAX_BODY_BYTES].decode("utf-8", "ignore")

    parts = [body.rstrip()]
    if truncated:
        parts.append("\n> ⚠ 输出过长已截断。用 pages / rows / columns / limit 收窄再读。")

    if result.warnings:
        lines = []
        for w in result.warnings:
            loc = w.get("location")
            where = f" @{loc['kind']}{loc['number']}" if loc else ""
            lines.append(f"- {w['code']}{where}: {w['message']}")
        parts.append("\n⚠ 完整性 warnings（请如实转达用户）：\n" + "\n".join(lines))

    s = result.stats
    page_info = f" · 总页数={s.page_count}" if s.page_count is not None else ""
    parts.append(f"\n〔meta〕来源={rel_path} · 格式={s.format} · 输出字节={s.output_bytes}{page_info}")

    if result.provenance and result.provenance.spans:
        spans = " ".join(
            f"p{sp['source']['number']}:[{sp['output']['start']},{sp['output']['end']})"
            for sp in result.provenance.spans[:12]
        )
        parts.append(f"〔provenance〕输出字节区间→源页：{spans}")

    return "\n".join(parts)


def extract_document_image(rel_path: str, uri: str) -> str:
    """提取内嵌媒体（spoor:// 占位符）→ 存到 .spoor-media/，供交给 VLM。"""
    abs_path = safe_resolve(rel_path)
    with open(abs_path, "rb") as f:
        data = f.read()
    blob = spoor.extract_media(data, uri, source_name=rel_path)

    out_dir = safe_resolve(".spoor-media")
    os.makedirs(out_dir, exist_ok=True)
    base = re.sub(r"[^a-zA-Z0-9._-]", "_", uri).lstrip("_")[-48:] or "media"
    name = f"{base}{_guess_ext(blob)}"
    with open(os.path.join(out_dir, name), "wb") as f:
        f.write(blob)

    return f"已提取内嵌资源 → .spoor-media/{name}（{_content_type(blob)}, {len(blob)} bytes）。可交给外部 VLM。"


def _content_type(b: bytes) -> str:
    if b[:4] == b"\x89PNG":
        return "image/png"
    if b[:3] == b"\xff\xd8\xff":
        return "image/jpeg"
    if b[:3] == b"GIF":
        return "image/gif"
    if b[:4] == b"RIFF" and b[8:12] == b"WEBP":
        return "image/webp"
    head = b[:64].decode("utf-8", "replace").strip().lower()
    if head.startswith("<?xml") or head.startswith("<svg"):
        return "image/svg+xml"
    return "application/octet-stream"


def _guess_ext(b: bytes) -> str:
    return {
        "image/png": ".png",
        "image/jpeg": ".jpg",
        "image/gif": ".gif",
        "image/webp": ".webp",
        "image/svg+xml": ".svg",
    }.get(_content_type(b), ".bin")


# —— 两个 spoor 工具的单一真相：schema + 派发。native 与 MCP server 共用。——
SPOOR_TOOLS: list[dict] = [
    {
        "name": "read_document",
        "description": (
            "读取 PDF/DOCX/XLSX/CSV/PPTX/EPUB/HTML 等文档，返回 LLM 可直接消费的文本"
            "（文档→Markdown，表格→JSON），并附完整性 warnings 与元信息。纯文本/代码文件请用 read_file。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "项目内文档路径，如 data/byd.pdf"},
                "pages": {"type": "array", "items": {"type": "number"}, "description": "[起,止] 1-based 闭区间，PDF 按页、PPTX 按幻灯片"},
                "sheet": {"type": "string", "description": "XLSX 工作表名"},
                "rows": {"type": "array", "items": {"type": "number"}, "description": "[起,止] 行区间；与 limit/offset 互斥"},
                "columns": {"type": "array", "items": {"type": "string"}, "description": "只保留这些列名"},
                "limit": {"type": "number", "description": "表格最多返回行数（默认 100）"},
                "offset": {"type": "number", "description": "跳过前 N 行"},
                "provenance": {"type": "string", "enum": ["page"], "description": "返回页级出处，便于把引用锚回原文"},
            },
            "required": ["path"],
        },
    },
    {
        "name": "extract_document_image",
        "description": "提取文档里的内嵌媒体（read_document 结果中出现的 spoor:// 占位符），存到 .spoor-media/ 供交给 VLM。",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "项目内文档路径"},
                "uri": {"type": "string", "description": "read_document 结果里的 spoor:// 占位符"},
            },
            "required": ["path", "uri"],
        },
    },
]


def run_spoor_tool(name: str, args: dict) -> str:
    """按名字派发一次 spoor 工具调用（native / mcp 共用；同步，pyspoor 直调）。"""
    args = args or {}
    if name == "read_document":
        return read_document(
            args["path"],
            pages=pair(args.get("pages")),
            sheet=opt_str(args.get("sheet")),
            rows=pair(args.get("rows")),
            columns=str_arr(args.get("columns")),
            limit=opt_num(args.get("limit")),
            offset=opt_num(args.get("offset")),
            provenance="page" if args.get("provenance") == "page" else None,
        )
    if name == "extract_document_image":
        return extract_document_image(args["path"], args["uri"])
    raise ValueError(f"未知的 spoor 工具: {name}")
