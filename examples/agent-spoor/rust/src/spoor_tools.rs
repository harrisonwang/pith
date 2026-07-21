//! spoor 能力的共享实现：native 工具与 MCP server 都调它，保证两条路结果一致。
//! 用 spoor-core（同进程、源头）。

use serde_json::{Value, json};
use spoor_core::{
    DocumentFilter, ParseContent, ParseRequest, ProvenanceLevel, SourceAnchor, TableFilter,
    WarningLocation, extract_media, parse,
};
use std::io::Write;

use crate::validate::{opt_str, opt_usize, pair, require_str, safe_resolve, str_vec};

const MAX_BODY_BYTES: usize = 96 * 1024;

/// 两个 spoor 工具的 OpenAI 风格 schema（native 列工具 + MCP server 建 rmcp Tool 都用它）。
pub fn spoor_tool_specs() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_document",
                "description": "读取 PDF/DOCX/XLSX/CSV/PPTX/EPUB/HTML 等文档，返回 LLM 可直接消费的文本（文档→Markdown，表格→JSON），并附完整性 warnings 与元信息。纯文本/代码文件请用 read_file。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "项目内文档路径，如 data/byd.pdf"},
                        "pages": {"type": "array", "items": {"type": "number"}, "description": "[起,止] 1-based 闭区间，PDF 按页、PPTX 按幻灯片"},
                        "sheet": {"type": "string", "description": "XLSX 工作表名"},
                        "rows": {"type": "array", "items": {"type": "number"}, "description": "[起,止] 行区间；与 limit/offset 互斥"},
                        "columns": {"type": "array", "items": {"type": "string"}, "description": "只保留这些列名"},
                        "limit": {"type": "number", "description": "表格最多返回行数（默认 100）"},
                        "offset": {"type": "number", "description": "跳过前 N 行"},
                        "provenance": {"type": "string", "enum": ["page"], "description": "返回页级出处，便于把引用锚回原文"}
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "extract_document_image",
                "description": "提取文档里的内嵌媒体（read_document 结果中出现的 spoor:// 占位符），存到 .spoor-media/ 供交给 VLM。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "项目内文档路径"},
                        "uri": {"type": "string", "description": "read_document 结果里的 spoor:// 占位符"}
                    },
                    "required": ["path", "uri"]
                }
            }
        }),
    ]
}

/// 按名字派发一次 spoor 工具调用（native / mcp 共用；同步，spoor-core 直调）。
pub fn run_spoor_tool(name: &str, args: &Value) -> String {
    let result = match name {
        "read_document" => read_document(args),
        "extract_document_image" => extract_document_image(args),
        other => Err(format!("未知的 spoor 工具: {other}")),
    };
    result.unwrap_or_else(|e| format!("错误: {e}"))
}

fn read_document(args: &Value) -> Result<String, String> {
    let rel = require_str(args, "path").map_err(|e| e.to_string())?;
    let abs = safe_resolve(rel).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&abs).map_err(|e| format!("读取失败: {e}"))?;

    let table_filter = TableFilter::build(
        opt_str(args, "sheet").map(str::to_string),
        pair(args, "rows"),
        str_vec(args, "columns"),
        opt_usize(args, "limit"),
        opt_usize(args, "offset"),
    )
    .map_err(|e| e.to_json())?;
    let document_filter = DocumentFilter::build(pair(args, "pages")).map_err(|e| e.to_json())?;

    let mut request = ParseRequest::new(&bytes);
    request.source_name = Some(rel);
    request.table_filter = table_filter;
    request.document_filter = document_filter;
    if opt_str(args, "provenance") == Some("page") {
        request.provenance = ProvenanceLevel::Page;
    }

    let result = parse(&request).map_err(|e| e.to_json())?;
    Ok(format_result(rel, &result))
}

fn format_result(rel: &str, result: &spoor_core::ParseResult) -> String {
    let mut body = match &result.content {
        ParseContent::Document(doc) => doc.markdown.clone(),
        ParseContent::Tables(tables) => {
            serde_json::to_string_pretty(&tables.tables).unwrap_or_else(|_| "[]".to_string())
        }
    };

    let mut truncated = false;
    if body.len() > MAX_BODY_BYTES {
        let mut end = MAX_BODY_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
        truncated = true;
    }

    let mut out = body.trim_end().to_string();
    if truncated {
        out.push_str("\n\n> ⚠ 输出过长已截断。用 pages / rows / columns / limit 收窄再读。");
    }

    if !result.warnings.is_empty() {
        out.push_str("\n\n⚠ 完整性 warnings（请如实转达用户）：");
        for w in &result.warnings {
            let loc = match &w.location {
                Some(WarningLocation::Page { number }) => format!(" @page{number}"),
                Some(WarningLocation::Slide { number }) => format!(" @slide{number}"),
                None => String::new(),
            };
            out.push_str(&format!("\n- {}{}: {}", w.code.as_str(), loc, w.message));
        }
    }

    let page_info = match result.stats.page_count {
        Some(n) => format!(" · 总页数={n}"),
        None => String::new(),
    };
    out.push_str(&format!(
        "\n\n〔meta〕来源={rel} · 格式={} · 输出字节={}{page_info}",
        result.stats.format, result.stats.output_bytes
    ));

    if let Some(prov) = &result.provenance
        && !prov.spans.is_empty()
    {
        let spans: Vec<String> = prov
            .spans
            .iter()
            .take(12)
            .map(|sp| {
                let source = match &sp.source {
                    SourceAnchor::Page { number, .. } => format!("p{number}"),
                    SourceAnchor::Slide { number } => format!("slide{number}"),
                    SourceAnchor::Input { start, end } => format!("input[{start},{end})"),
                    SourceAnchor::Cell { sheet, row, column } => match sheet {
                        Some(sheet) => format!("cell[{sheet}!{column}:{row}]"),
                        None => format!("cell[{column}:{row}]"),
                    },
                };
                format!("{source}:[{},{})", sp.output.start, sp.output.end)
            })
            .collect();
        out.push_str(&format!(
            "\n〔provenance〕输出字节区间→源锚点：{}",
            spans.join(" ")
        ));
    }

    out
}

fn extract_document_image(args: &Value) -> Result<String, String> {
    let rel = require_str(args, "path").map_err(|e| e.to_string())?;
    let uri = require_str(args, "uri").map_err(|e| e.to_string())?;
    let abs = safe_resolve(rel).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&abs).map_err(|e| format!("读取失败: {e}"))?;

    let mut request = ParseRequest::new(&bytes);
    request.source_name = Some(rel);
    let media = extract_media(&request, uri).map_err(|e| e.to_json())?;

    let out_dir = safe_resolve(".spoor-media").map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    // 全部映射成 ASCII 安全字符，取尾部 48 个（都是 ASCII，按字节切安全）。
    let mut base: String = uri
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    base = base.trim_start_matches('_').to_string();
    if base.len() > 48 {
        base = base[base.len() - 48..].to_string();
    }
    if base.is_empty() {
        base = "media".to_string();
    }
    let name = format!("{base}{}", ext(&media));
    let path = out_dir.join(&name);
    std::fs::File::create(&path)
        .and_then(|mut f| f.write_all(&media))
        .map_err(|e| e.to_string())?;

    Ok(format!(
        "已提取内嵌资源 → .spoor-media/{name}（{}, {} bytes）。可交给外部 VLM。",
        content_type(&media),
        media.len()
    ))
}

fn content_type(b: &[u8]) -> &'static str {
    if b.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        "image/png"
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if b.starts_with(b"GIF") {
        "image/gif"
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        let head = String::from_utf8_lossy(&b[..b.len().min(64)]);
        let head = head.trim_start().to_ascii_lowercase();
        if head.starts_with("<?xml") || head.starts_with("<svg") {
            "image/svg+xml"
        } else {
            "application/octet-stream"
        }
    }
}

fn ext(b: &[u8]) -> &'static str {
    match content_type(b) {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/svg+xml" => ".svg",
        _ => ".bin",
    }
}
