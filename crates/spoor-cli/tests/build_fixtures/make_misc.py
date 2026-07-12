#!/usr/bin/env python3
"""Misc fixture builders: pdf, epub, plain, adversarial."""
import base64
from io import BytesIO
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parent.parent / "fixtures"


# ============================================================
# PDF — using reportlab to make a few simple text-layer PDFs
# ============================================================
def build_pdfs():
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
    except ImportError:
        print("reportlab not installed - skipping PDFs")
        return

    # 01: simple single-page PDF
    c = canvas.Canvas(str(out / "01_basic.pdf"), pagesize=letter)
    c.setFont("Helvetica", 14)
    c.drawString(72, 720, "Document title")
    c.setFont("Helvetica", 11)
    c.drawString(72, 690, "First paragraph of the document body.")
    c.drawString(72, 670, "Second paragraph follows.")
    c.showPage()
    c.save()

    # 02: multi-page
    c = canvas.Canvas(str(out / "02_multipage.pdf"), pagesize=letter)
    for i in range(1, 4):
        c.setFont("Helvetica", 12)
        c.drawString(72, 720, f"Page {i} content begins here.")
        c.drawString(72, 700, "Some text on this page.")
        c.showPage()
    c.save()

    # 03: unicode / Chinese (uses default font, may drop chars - intentional;
    # this fixture verifies graceful handling, not perfect output).
    c = canvas.Canvas(str(out / "03_ascii_only.pdf"), pagesize=letter)
    c.setFont("Helvetica", 12)
    c.drawString(72, 720, "ASCII only PDF for baseline test.")
    c.showPage()
    c.save()

    build_image_only_pdf()


def build_image_only_pdf():
    """Build a PDF containing an image object but no text layer."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
        from reportlab.lib.utils import ImageReader
    except ImportError:
        print("reportlab not installed - skipping image-only PDF")
        return

    png = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk"
        "+A8AAQUBAScY42YAAAAASUVORK5CYII="
    )
    c = canvas.Canvas(str(out / "04_image_only.pdf"), pagesize=letter)
    c.drawImage(ImageReader(BytesIO(png)), 72, 620, width=300, height=180)
    c.showPage()
    c.save()


def build_mixed_text_and_image_pdf():
    """Build a mixed PDF where only one page lacks a text layer."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
        from reportlab.lib.utils import ImageReader
    except ImportError:
        print("reportlab not installed - skipping mixed PDF")
        return

    png = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk"
        "+A8AAQUBAScY42YAAAAASUVORK5CYII="
    )
    c = canvas.Canvas(str(out / "05_mixed_text_and_image.pdf"), pagesize=letter)
    c.drawString(72, 720, "This page has a text layer.")
    c.showPage()
    c.drawImage(ImageReader(BytesIO(png)), 72, 620, width=300, height=180)
    c.showPage()
    c.save()


def build_links_pdf():
    """08: URI link annotations — anchored, unanchored, and unsafe-scheme."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
        from reportlab.pdfbase.pdfmetrics import stringWidth
    except ImportError:
        print("reportlab not installed - skipping links PDF")
        return

    c = canvas.Canvas(str(out / "08_links.pdf"), pagesize=letter)
    c.setFont("Helvetica", 12)

    # Anchored link: annotation rect sits exactly over "full guide".
    prefix, anchor, suffix = "See the ", "full guide", " for details."
    x, y = 72, 700
    c.drawString(x, y, prefix + anchor + suffix)
    ax0 = x + stringWidth(prefix, "Helvetica", 12)
    ax1 = ax0 + stringWidth(anchor, "Helvetica", 12)
    c.linkURL("https://example.com/guide", (ax0, y - 3, ax1, y + 11), relative=0)

    # Unsafe scheme over real text: must be dropped, text left unwrapped.
    js_text = "Do not execute this."
    c.drawString(x, 660, js_text)
    js_w = stringWidth(js_text, "Helvetica", 12)
    c.linkURL("javascript:alert(1)", (x, 657, x + js_w, 671), relative=0)

    # Unanchored link: rect over empty page area, target must still survive.
    c.linkURL("https://example.com/api", (x, 600, x + 150, 615), relative=0)

    c.showPage()
    c.save()


def build_outline_pdf():
    """09: document outline (bookmarks) naming heading lines on two pages."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
    except ImportError:
        print("reportlab not installed - skipping outline PDF")
        return

    c = canvas.Canvas(str(out / "09_outline.pdf"), pagesize=letter)
    c.setFont("Helvetica", 12)

    c.drawString(72, 720, "Introduction")
    c.drawString(72, 700, "Opening prose that follows the first heading.")
    c.drawString(72, 660, "Background")
    c.drawString(72, 640, "More prose under the nested heading.")
    c.bookmarkPage("intro")
    c.addOutlineEntry("Introduction", "intro", level=0)
    c.bookmarkPage("background")
    c.addOutlineEntry("Background", "background", level=1)
    c.showPage()

    c.setFont("Helvetica", 12)
    c.drawString(72, 720, "Methods")
    c.drawString(72, 700, "Second page prose paragraph.")
    c.bookmarkPage("methods")
    c.addOutlineEntry("Methods", "methods", level=0)
    # An outline entry whose title appears nowhere on the page: spoor must
    # not fabricate a heading for it. Distinct key — reportlab dedupes
    # outline entries by key, so sharing "methods" would drop that entry.
    c.bookmarkPage("missing")
    c.addOutlineEntry("Missing Section", "missing", level=1)
    c.showPage()
    c.save()


def build_header_footer_pdf():
    """10: repeated header/footer plus per-page page numbers across 4 pages."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
    except ImportError:
        print("reportlab not installed - skipping header/footer PDF")
        return

    c = canvas.Canvas(str(out / "10_header_footer.pdf"), pagesize=letter)
    bodies = [
        "Revenue grew steadily across the first quarter.",
        "Costs were kept flat by renegotiated contracts.",
        "The outlook section describes second half risks.",
        "Appendix tables list the full segment breakdown.",
    ]
    for number, body in enumerate(bodies, start=1):
        c.setFont("Helvetica", 9)
        c.drawString(72, 750, "ACME Corp Annual Report 2026")
        c.setFont("Helvetica", 12)
        c.drawString(72, 700, body)
        c.setFont("Helvetica", 9)
        c.drawString(280, 40, f"Page {number} of 4")
        c.showPage()
    c.save()


def build_hyphenation_pdf():
    """11: line-end hyphenated words that must be rejoined, plus guards."""
    out = ROOT / "pdf"
    out.mkdir(parents=True, exist_ok=True)
    try:
        from reportlab.pdfgen import canvas
        from reportlab.lib.pagesizes import letter
    except ImportError:
        print("reportlab not installed - skipping hyphenation PDF")
        return

    c = canvas.Canvas(str(out / "11_hyphenation.pdf"), pagesize=letter)
    c.setFont("Helvetica", 12)
    lines = [
        # Broken word: "dehyphen-" + "ation" must rejoin as "dehyphenation".
        "The parser applies a conservative dehyphen-",
        "ation pass to line ends before rendering.",
        # Hyphenated compound broken at an inner hyphen: rejoining keeps it.
        "The result reads like state-of-",
        "the-art extraction output.",
        # Guards that must NOT be joined:
        "A trailing minus stays: total = subtotal -",
        "discount applies on the following line.",
        "A capitalized continuation stays split: UTF-",
        "8 style acronyms keep their hyphen.",
    ]
    # Tight leading (14pt at 12pt font) mirrors real justified body text;
    # wider gaps extract as paragraph breaks, which dehyphenation must not
    # join across.
    y = 720
    for line in lines:
        c.drawString(72, y, line)
        y -= 14
    c.showPage()
    c.save()


# ============================================================
# EPUB — minimal but correct structure (container + OPF + spine)
# ============================================================
def build_epubs():
    out = ROOT / "epub"
    out.mkdir(parents=True, exist_ok=True)

    # 01: a real epub with two chapters and proper spine ordering
    container = """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"""
    opf = """<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>Test Book</dc:title>
<dc:creator>Test Author</dc:creator>
<dc:identifier id="bookid">urn:test:001</dc:identifier>
<dc:language>en</dc:language>
</metadata>
<manifest>
<item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
<item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
</manifest>
<spine>
<itemref idref="ch1"/>
<itemref idref="ch2"/>
</spine>
</package>"""
    ch1 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter 1</title></head><body>
<h1>Chapter One</h1>
<p>This is the first chapter, with a <strong>bold</strong> word.</p>
<ul><li>Item A</li><li>Item B</li></ul>
</body></html>"""
    ch2 = """<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter 2</title></head><body>
<h1>Chapter Two</h1>
<p>Second chapter content.</p>
</body></html>"""

    with zipfile.ZipFile(out / "01_basic.epub", "w") as z:
        # mimetype must be the first entry, uncompressed
        z.writestr(zipfile.ZipInfo("mimetype"), "application/epub+zip",
                   compress_type=zipfile.ZIP_STORED)
        z.writestr("META-INF/container.xml", container)
        z.writestr("OEBPS/content.opf", opf)
        z.writestr("OEBPS/ch1.xhtml", ch1)
        z.writestr("OEBPS/ch2.xhtml", ch2)


# ============================================================
# Plain text — encoding variants
# ============================================================
def build_plains():
    out = ROOT / "plain"
    out.mkdir(parents=True, exist_ok=True)
    (out / "01_ascii.txt").write_text("Hello world\nLine two\n", encoding="utf-8")
    (out / "02_utf8.txt").write_text("中文 UTF-8\n日本語\n한글\n", encoding="utf-8")
    (out / "03_gbk.txt").write_bytes("中文 GBK 编码\n第二行\n".encode("gbk"))
    (out / "04_utf16le_bom.txt").write_bytes(
        b"\xff\xfe" + "UTF-16 LE with BOM\nLine 2\n".encode("utf-16-le")
    )
    (out / "05_code.py").write_text(
        "def hello(name):\n    print(f'Hello, {name}')\n\nhello('world')\n",
        encoding="utf-8",
    )


# ============================================================
# Adversarial — broken inputs that should fail cleanly
# ============================================================
def build_adversarial():
    out = ROOT / "adversarial"
    out.mkdir(parents=True, exist_ok=True)
    (out / "01_empty.docx").write_bytes(b"")
    (out / "02_not_zip.docx").write_text("this is not a zip file at all")
    (out / "03_truncated_zip.docx").write_bytes(b"PK\x03\x04" + b"\x00" * 50)
    # Broken JSON
    (out / "04_broken.ipynb").write_text("{not valid json")
    # Bomb-ish: a tiny zip that claims to decompress to gigabytes.
    # Builds a docx where document.xml is highly compressible.
    big_xml = ('<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
               '<w:body><w:p><w:r><w:t>' + ("A" * (5 * 1024 * 1024)) + '</w:t></w:r></w:p></w:body></w:document>')
    types = ('<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
             '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
    rels = ('<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
    with zipfile.ZipFile(out / "05_compression_bomb.docx", "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", types)
        z.writestr("_rels/.rels", rels)
        z.writestr("word/document.xml", big_xml)


if __name__ == "__main__":
    build_pdfs()
    build_links_pdf()
    build_outline_pdf()
    build_header_footer_pdf()
    build_hyphenation_pdf()
    build_epubs()
    build_plains()
    build_adversarial()
    print("done")
