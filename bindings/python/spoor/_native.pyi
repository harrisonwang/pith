from __future__ import annotations

from typing import Any

class SpoorError(Exception): ...

def parse_bytes(
    data: bytes,
    source_name: str | None,
    content_type: str | None,
    format: str | None,
    max_parse_bytes: int | None,
    sheet: str | None,
    rows: tuple[int, int] | None,
    columns: list[str] | None,
    limit: int | None,
    offset: int | None,
    pages: tuple[int, int] | None,
    max_work_units: int | None,
    provenance: str | None,
    keep_repeated_regions: bool | None,
) -> dict[str, Any]: ...
def extract_media(
    data: bytes,
    resource: str,
    source_name: str | None,
    content_type: str | None,
    format: str | None,
    max_parse_bytes: int | None,
) -> bytes: ...
def detect_format(
    data: bytes,
    source_name: str | None,
    content_type: str | None,
) -> str: ...
def locate_quote(
    markdown: str,
    quote: str,
) -> dict[str, Any] | None: ...
