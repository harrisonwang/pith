"""spoor.answer-trace.v1 — Python 端契约(pydantic v2)。

前端 TS 见 ../src/index.ts;权威 JSON Schema 见 ../answer-trace.schema.json。
phase 1 的 api 直接转发 fixture JSON,这些模型留给 phase 2 的 matcher(经
Cloudflare Workers AI 调生成/判定模型)产出 AnswerTrace 时做校验与序列化。
"""

from __future__ import annotations

from typing import Annotated, Literal, Union

from pydantic import BaseModel, Field

SCHEMA_VERSION = "spoor.answer-trace.v1"

Verdict = Literal["supported", "partial", "unsupported"]


class TextPart(BaseModel):
    type: Literal["text"]
    text: str


class ClaimPart(BaseModel):
    type: Literal["claim"]
    text: str
    verdict: Verdict
    evidenceIds: list[str]


AnswerPart = Annotated[Union[TextPart, ClaimPart], Field(discriminator="type")]


class Span(BaseModel):
    start: int
    end: int


class Bbox(BaseModel):
    x0: float
    y0: float
    x1: float
    y1: float


class EvidenceAnchor(BaseModel):
    doc: int | None = None
    page: int
    bbox: Bbox | None = None


class QuoteEvidence(BaseModel):
    id: str
    kind: Literal["quote"]
    verdict: Verdict
    page: int | None
    before: str
    hit: str
    after: str
    span: Span | None = None
    anchor: EvidenceAnchor | None = None
    note: str | None = None


class CellEvidence(BaseModel):
    id: str
    kind: Literal["cell"]
    verdict: Verdict
    page: int | None
    table: str
    row: str
    column: str
    value: str
    note: str | None = None


class NoEvidence(BaseModel):
    id: str
    kind: Literal["none"]
    verdict: Literal["unsupported"]
    page: int | None = None
    reason: str
    expectedTruth: str | None = None
    note: str | None = None


Evidence = Annotated[
    Union[QuoteEvidence, CellEvidence, NoEvidence], Field(discriminator="kind")
]


class SourceRef(BaseModel):
    documentId: str
    title: str
    pages: int | None = None


class AuditInfo(BaseModel):
    parser: str
    generator: str
    judge: str
    judgedAt: str


class AnswerTrace(BaseModel):
    schema_: Literal["spoor.answer-trace.v1"] = Field(alias="schema")
    question: str
    answer: list[AnswerPart]
    evidence: list[Evidence]
    source: SourceRef
    audit: AuditInfo
