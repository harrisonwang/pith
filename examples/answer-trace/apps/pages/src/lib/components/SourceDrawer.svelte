<script lang="ts">
  import { marked } from 'marked'
  import type { EvidenceAnchor, Verdict } from '@answer-trace/protocol'

  let {
    open,
    markdown,
    locate,
    status,
    title,
    page,
    anchor = null,
    corpusId = null,
    onClose,
  }: {
    open: boolean
    markdown: string
    locate: string | null
    status: Verdict
    title: string
    page: number | null
    anchor?: EvidenceAnchor | null
    corpusId?: string | null
    onClose: () => void
  } = $props()

  marked.setOptions({ gfm: true, breaks: false })

  // 只渲染命中所在那一页(大文档下钻才不卡);无页码则整篇。
  function slicePage(md: string, p: number): string {
    const m = new RegExp(`(?:^|\\n)##[ \\t]*Page[ \\t]+${p}\\b`).exec(md)
    if (!m) return md
    const headStart = m.index + (md[m.index] === '\n' ? 1 : 0)
    const next = md.indexOf('\n## Page ', headStart + 1)
    return md.slice(headStart, next === -1 ? md.length : next)
  }

  const rendered = $derived(page != null && markdown ? slicePage(markdown, page) : markdown)
  const html = $derived(rendered ? (marked.parse(rendered) as string) : '')

  let contentEl = $state<HTMLElement | null>(null)

  // —— 原 PDF 页渲染 + 证据框(证据带块级 anchor 时) ——
  let pdfWrap = $state<HTMLDivElement | null>(null)
  let pdfCanvas = $state<HTMLCanvasElement | null>(null)
  let pdfStatus = $state<'idle' | 'loading' | 'ready' | 'failed'>('idle')
  let pdfBox = $state<{ left: number; top: number; width: number; height: number } | null>(null)

  $effect(() => {
    if (!open || !anchor?.bbox || !pdfCanvas || !pdfWrap) {
      pdfStatus = 'idle'
      pdfBox = null
      return
    }
    let cancelled = false
    pdfStatus = 'loading'
    renderPdfPage(anchor, pdfCanvas, pdfWrap)
      .then((box) => {
        if (cancelled) return
        pdfBox = box
        pdfStatus = 'ready'
      })
      .catch(() => {
        if (!cancelled) pdfStatus = 'failed' // 静默降级:下方 markdown 高亮仍然可用
      })
    return () => {
      cancelled = true
    }
  })

  async function renderPdfPage(
    a: EvidenceAnchor,
    canvas: HTMLCanvasElement,
    wrap: HTMLDivElement,
  ): Promise<{ left: number; top: number; width: number; height: number } | null> {
    const pdfjs = await import('pdfjs-dist')
    const worker = await import('pdfjs-dist/build/pdf.worker.min.mjs?url')
    pdfjs.GlobalWorkerOptions.workerSrc = worker.default

    const params = new URLSearchParams()
    if (corpusId) params.set('corpus', corpusId)
    params.set('doc', String(a.doc ?? 0))
    const res = await fetch(`/api/raw?${params}`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.arrayBuffer()

    const doc = await pdfjs.getDocument({ data }).promise
    try {
      const pdfPage = await doc.getPage(a.page)
      const base = pdfPage.getViewport({ scale: 1 })
      const cssWidth = wrap.clientWidth || 600
      const dpr = Math.min(globalThis.devicePixelRatio || 1, 2)
      const viewport = pdfPage.getViewport({ scale: (cssWidth / base.width) * dpr })
      canvas.width = Math.ceil(viewport.width)
      canvas.height = Math.ceil(viewport.height)
      canvas.style.width = `${Math.ceil(viewport.width / dpr)}px`
      canvas.style.height = `${Math.ceil(viewport.height / dpr)}px`
      const context = canvas.getContext('2d')
      if (!context) throw new Error('no 2d context')
      await pdfPage.render({ canvasContext: context, viewport }).promise

      if (!a.bbox) return null
      // anchor.bbox 是 PDF 原生用户空间(y 向上),viewport 直接换算成画布坐标。
      const [x0, y0, x1, y1] = viewport.convertToViewportRectangle([
        a.bbox.x0,
        a.bbox.y0,
        a.bbox.x1,
        a.bbox.y1,
      ])
      const pad = 3
      return {
        left: Math.min(x0, x1) / dpr - pad,
        top: Math.min(y0, y1) / dpr - pad,
        width: Math.abs(x1 - x0) / dpr + pad * 2,
        height: Math.abs(y1 - y0) / dpr + pad * 2,
      }
    } finally {
      void doc.destroy()
    }
  }

  // 打开 + 有目标时:等抽屉滑入后,在渲染好的原文里定位命中、高亮、滚动、闪烁。
  $effect(() => {
    if (!open || !locate || !contentEl) return
    const root = contentEl
    const needle = locate
    const cls = status
    const t = setTimeout(() => highlight(root, needle, cls), 300)
    return () => clearTimeout(t)
  })

  function highlight(root: HTMLElement, needle: string, cls: string) {
    // 清掉上次高亮
    root.querySelectorAll('mark.drawer-hl').forEach((m) => {
      m.replaceWith(document.createTextNode(m.textContent ?? ''))
    })
    root.normalize()

    const target = needle.replace(/\s+/g, '')
    if (!target) return

    // 把所有文本节点拼成"无空白串",记录每个字符回到 (node, offset) 的映射。
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
    const map: { node: Text; offset: number }[] = []
    let combined = ''
    let node: Node | null
    while ((node = walker.nextNode())) {
      const data = (node as Text).data
      for (let i = 0; i < data.length; i++) {
        if (/\s/.test(data[i])) continue
        map.push({ node: node as Text, offset: i })
        combined += data[i]
      }
    }

    const idx = combined.indexOf(target)
    if (idx === -1) return
    const start = map[idx]
    const end = map[idx + target.length - 1]

    const range = document.createRange()
    range.setStart(start.node, start.offset)
    range.setEnd(end.node, end.offset + 1)

    const mark = document.createElement('mark')
    mark.className = `drawer-hl ${cls}`
    try {
      range.surroundContents(mark)
      mark.scrollIntoView({ behavior: 'smooth', block: 'center' })
      mark.classList.add('flash')
      setTimeout(() => mark.classList.remove('flash'), 1200)
    } catch {
      // 命中跨越元素边界,无法整体包裹 → 退而求其次,滚到起点
      ;(start.node.parentElement ?? root).scrollIntoView({ behavior: 'smooth', block: 'center' })
    }
  }
</script>

<div
  class="fixed inset-0 z-50 {open ? 'pointer-events-auto' : 'pointer-events-none'}"
  aria-hidden={!open}
>
  <button
    type="button"
    aria-label="关闭"
    class="absolute inset-0 cursor-default bg-[#1A1D21]/20 transition-opacity duration-200 {open
      ? 'opacity-100'
      : 'opacity-0'}"
    onclick={onClose}
  ></button>

  <aside
    class="absolute top-0 right-0 flex h-full w-full max-w-[680px] flex-col border-l border-[#E7E9EC] bg-white shadow-[0_24px_80px_rgba(16,24,40,0.18)] transition-transform duration-200 ease-out {open
      ? 'translate-x-0'
      : 'translate-x-full'}"
  >
    <div class="flex h-16 shrink-0 items-center justify-between border-b border-[#E7E9EC] px-5">
      <div class="min-w-0">
        <div class="text-[13px] font-semibold text-[#1A1D21]">原文下钻</div>
        <div class="mt-0.5 truncate text-[12px] text-[#5B6370]">
          {title}{page != null ? ` · 第 ${page} 页` : ''} · 命中位置已自动定位
        </div>
      </div>
      <button
        type="button"
        onclick={onClose}
        class="grid h-9 w-9 place-items-center rounded-lg border border-[#E7E9EC] text-[#5B6370] transition hover:bg-[#F7F8FA] hover:text-[#1A1D21]"
        aria-label="关闭">✕</button
      >
    </div>

    <div class="flex-1 overflow-y-auto">
      {#if anchor?.bbox}
        <div class="border-b border-[#E7E9EC] bg-[#FBFBFC] px-6 py-5">
          <div class="mb-2 flex items-center justify-between text-[12px] text-[#5B6370]">
            <span>原始 PDF · 第 {anchor.page} 页(证据位置已框出)</span>
            {#if pdfStatus === 'loading'}<span>渲染中…</span>{/if}
            {#if pdfStatus === 'failed'}<span>原 PDF 暂不可取,以下按解析文本定位</span>{/if}
          </div>
          <div
            bind:this={pdfWrap}
            class="relative overflow-hidden rounded-lg border border-[#E7E9EC] bg-white {pdfStatus ===
            'ready'
              ? ''
              : 'min-h-24'}"
          >
            <canvas bind:this={pdfCanvas} class={pdfStatus === 'ready' ? 'block' : 'hidden'}
            ></canvas>
            {#if pdfBox && pdfStatus === 'ready'}
              <div
                class="pointer-events-none absolute rounded-sm border-2 {status === 'supported'
                  ? 'border-[#12B76A]/80 bg-[#12B76A]/10'
                  : 'border-[#F79009]/80 bg-[#F79009]/10'}"
                style="left:{pdfBox.left}px;top:{pdfBox.top}px;width:{pdfBox.width}px;height:{pdfBox.height}px"
              ></div>
            {/if}
          </div>
        </div>
      {/if}

      <div bind:this={contentEl} class="drawer-md px-6 py-6">
        <!-- 渲染的是真实 spoor 产物(byd.md 或上传文件经 pyspoor 解析的 markdown) -->
        {@html html}
      </div>
    </div>
  </aside>
</div>
