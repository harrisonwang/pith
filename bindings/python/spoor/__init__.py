from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from . import _native
from .exceptions import SpoorError
from .types import (
    LocatedQuote,
    LocateMethod,
    ParseResult,
    Provenance,
    ProvenanceSpan,
    QuoteSpan,
    SourceAnchor,
    SpoorWarning,
    TextRange,
    WarningCode,
    WarningLocation,
    parse_result,
)

__all__ = [
    "LocateMethod",
    "LocatedQuote",
    "ParseResult",
    "Provenance",
    "ProvenanceSpan",
    "QuoteSpan",
    "SourceAnchor",
    "SpoorError",
    "SpoorWarning",
    "TextRange",
    "WarningCode",
    "WarningLocation",
    "detect_format",
    "extract_media",
    "locate_quote",
    "parse_bytes",
    "parse_path",
]


def parse_bytes(
    data: bytes,
    *,
    source_name: str | None = None,
    content_type: str | None = None,
    format: str | None = None,
    max_parse_bytes: int | None = None,
    sheet: str | None = None,
    rows: tuple[int, int] | None = None,
    columns: list[str] | None = None,
    limit: int | None = None,
    offset: int | None = None,
    pages: tuple[int, int] | None = None,
    max_work_units: int | None = None,
    provenance: str | None = None,
    keep_repeated_regions: bool = False,
) -> ParseResult:
    """Parse document/table bytes into a typed result.

    For table formats (CSV/XLSX) the narrowing options mirror the CLI: ``sheet``
    (XLSX only), ``rows`` as an inclusive 1-based ``(first, last)`` pair (mutually
    exclusive with ``limit``/``offset``), ``columns`` to keep, and
    ``limit``/``offset`` for pagination. For page-oriented formats (PDF pages, PPTX slides), ``pages``
    is an inclusive 1-based ``(first, last)`` range that limits which pages are
    parsed. Each option is ignored by formats it does not apply to.

    ``provenance="page"`` returns a page/slide-level mapping and ``"block"``
    requests the finest available mapping. Output byte ranges index
    ``markdown`` as UTF-8, so slice with
    ``markdown.encode("utf-8")[start:end]``.

    PDF cross-page repeated headers/footers are deduplicated by default (first
    occurrence kept, warning ``pdf_repeated_region_deduplicated`` names what
    moved); pass ``keep_repeated_regions=True`` for verbatim page text.
    """
    try:
        raw: dict[str, Any] = _native.parse_bytes(
            data,
            source_name,
            content_type,
            format,
            max_parse_bytes,
            sheet,
            rows,
            columns,
            limit,
            offset,
            pages,
            max_work_units,
            provenance,
            keep_repeated_regions,
        )
    except _native.SpoorError as error:
        raise SpoorError.from_native(error) from None
    return parse_result(raw)


def parse_path(
    path: str | Path,
    *,
    format: str | None = None,
    max_parse_bytes: int | None = None,
    sheet: str | None = None,
    rows: tuple[int, int] | None = None,
    columns: list[str] | None = None,
    limit: int | None = None,
    offset: int | None = None,
    pages: tuple[int, int] | None = None,
    max_work_units: int | None = None,
    provenance: str | None = None,
    keep_repeated_regions: bool = False,
) -> ParseResult:
    path = Path(path)
    return parse_bytes(
        path.read_bytes(),
        source_name=str(path),
        format=format,
        max_parse_bytes=max_parse_bytes,
        sheet=sheet,
        rows=rows,
        columns=columns,
        limit=limit,
        offset=offset,
        pages=pages,
        max_work_units=max_work_units,
        provenance=provenance,
        keep_repeated_regions=keep_repeated_regions,
    )


def extract_media(
    data: bytes,
    resource: str,
    *,
    source_name: str | None = None,
    content_type: str | None = None,
    format: str | None = None,
    max_parse_bytes: int | None = None,
) -> bytes:
    """Extract one safe embedded media resource referenced by a URI spoor emitted.

    ``resource`` is a safe URI from the parsed output, e.g.
    ``spoor://docx/part/word/media/image1.png``,
    ``spoor://pptx/part/ppt/media/imageN.png``, or
    ``spoor://pdf/obj/{id}/{gen}``.
    Returns the raw bytes; spoor does not decode or interpret them.
    """
    try:
        return _native.extract_media(
            data, resource, source_name, content_type, format, max_parse_bytes
        )
    except _native.SpoorError as error:
        raise SpoorError.from_native(error) from None


def locate_quote(
    markdown: str,
    quote: str,
    provenance: Provenance | list[dict[str, Any]] | None = None,
) -> LocatedQuote | None:
    """Ground an LLM-cited quote in Markdown spoor produced.

    Tries five deterministic tiers, strictest first: exact substring,
    normalized (``whitespace_insensitive``: whitespace, full/half-width and
    CJK punctuation variants, list/heading markers, link syntax), fuzzy
    (bounded edit distance with a similarity ``score`` and a hard constraint
    that every figure in the quote appears in the match), table-cell anchor,
    and numeric/unit equivalence. The last two return a source candidate, not
    a verbatim quote; ``corroborated=False`` marks a candidate accepted on
    value uniqueness alone. ``occurrences > 1`` means the location is one of
    several plausible ones. ``None`` only means no tier matched the supplied
    Markdown; scans, visuals, or parse omissions may still contain the
    content. A match does not establish that the material supports the claim
    or that the claim is true.

    ``result.span`` indexes ``markdown`` as a Python ``str``:
    ``markdown[span["start"]:span["end"]]`` is the raw hit. ``result.page`` is
    read from spoor's own ``## Page N`` markers when present.

    Pass ``provenance`` (the ``result.provenance`` of the same parse, or its
    ``spans`` list) to also fill ``result.anchor`` with the source anchor of
    the best-overlapping span — with block-level provenance that is the
    quote's approximate box on its source page.
    """
    spans_json: str | None = None
    if provenance is not None:
        spans = getattr(provenance, "spans", provenance)
        spans_json = json.dumps(spans)
    raw = _native.locate_quote(markdown, quote, spans_json)
    if raw is None:
        return None
    return LocatedQuote(
        span=raw["span"],
        before=raw["before"],
        hit=raw["hit"],
        after=raw["after"],
        page=raw["page"],
        method=raw["method"],
        score=raw.get("score"),
        occurrences=raw["occurrences"],
        corroborated=raw["corroborated"],
        anchor=raw.get("anchor"),
    )


def detect_format(
    data: bytes,
    *,
    source_name: str | None = None,
    content_type: str | None = None,
) -> str:
    try:
        return _native.detect_format(data, source_name, content_type)
    except _native.SpoorError as error:
        raise SpoorError.from_native(error) from None
