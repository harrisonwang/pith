<script lang="ts">
  import { onMount } from 'svelte'
  import type { AnswerTrace, Evidence, EvidenceAnchor, Verdict } from '@answer-trace/protocol'
  import ChatRound from '$lib/components/ChatRound.svelte'
  import SourceDrawer from '$lib/components/SourceDrawer.svelte'
  import { askQuestion, currentCorpus, uploadFiles } from '$lib/loadTrace'

  interface Round {
    trace: AnswerTrace
    sourceMarkdown: string
    title: string
  }

  let rounds = $state<Round[]>([])

  // 当前依据(语料)。空初始态,上传后切到上传文件。
  let currentMarkdown = $state('')
  let currentTitle = $state('')
  let currentTokens = $state(0)
  let currentContextLimit = $state(32768)

  // 是否已准备就绪(有语料 + 可提问)
  let ready = $derived(!!currentMarkdown)
  let uploadedFiles = $state<string[]>([])

  let input = $state('')
  let asking = $state(false)
  let askError = $state<string | null>(null)
  let uploading = $state(false)
  let uploadError = $state<string | null>(null)

  let fileInput = $state<HTMLInputElement | null>(null)

  let drawer = $state<{
    open: boolean
    markdown: string
    locate: string | null
    status: Verdict
    title: string
    page: number | null
    anchor: EvidenceAnchor | null
    corpusId: string | null
  }>({
    open: false,
    markdown: '',
    locate: null,
    status: 'supported',
    title: '',
    page: null,
    anchor: null,
    corpusId: null,
  })

  onMount(() => {
    // 空初始态,不加载演示数据。用户上传文档后开始提问。
  })

  function openDrawer(round: Round, ev: Evidence) {
    const locate = ev.kind === 'quote' ? ev.hit : ev.kind === 'cell' ? ev.value : null
    if (!locate) return
    drawer = {
      open: true,
      markdown: round.sourceMarkdown,
      locate,
      status: ev.verdict,
      title: round.title,
      page: ev.page ?? null,
      // quote 证据带 anchor(块级 provenance 命中)时,抽屉可回原 PDF 画框。
      anchor: ev.kind === 'quote' ? (ev.anchor ?? null) : null,
      corpusId: currentCorpus(),
    }
  }

  async function submitAsk() {
    const q = input.trim()
    if (!q || asking) return
    asking = true
    askError = null
    input = ''
    try {
      const trace = await askQuestion(q)
      rounds = [...rounds, { trace, sourceMarkdown: currentMarkdown, title: currentTitle }]
    } catch (e) {
      askError = e instanceof Error ? e.message : String(e)
    } finally {
      asking = false
    }
  }

  async function onUpload(files: File[]) {
    if (!files.length || uploading) return
    uploading = true
    uploadError = null
    try {
      const res = await uploadFiles(files)
      currentMarkdown = res.markdown
      currentTitle = res.source.title
      currentTokens = res.tokens
      currentContextLimit = res.contextLimit
      uploadedFiles = res.files.filter((f) => f.ok).map((f) => f.name)
      const failed = res.files.filter((f) => !f.ok)
      if (failed.length) uploadError = `部分文件解析失败:${failed.map((f) => f.name).join('、')}`
    } catch (e) {
      uploadError = e instanceof Error ? e.message : String(e)
    } finally {
      uploading = false
    }
  }
</script>

<div class="min-h-screen bg-[#F7F8FA] text-[#1A1D21]">
  <main class="mx-auto max-w-[760px] px-4 pt-8 pb-40">
    <div class="space-y-8">
      {#if rounds.length === 0}
        <div class="flex flex-col items-center justify-center py-24 text-[13px] text-[#9AA1A9]">
          <span class="mb-4 text-[40px]">📂</span>
          <p class="text-[15px] font-medium text-[#5B6370]">上传文档开始提问</p>
          <p class="mt-1 text-[12px]">支持 PDF，经 spoor 解析后可逐 claim 溯源</p>
        </div>
      {/if}
      {#each rounds as round, i (i)}
        <ChatRound trace={round.trace} onOpenSource={(ev) => openDrawer(round, ev)} />
      {/each}
      {#if asking}
        <div class="flex items-center gap-2 pl-10 text-[13px] text-[#9AA1A9]">
          <span
            class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-[#CBD2DA] border-t-[#2563EB]"
          ></span>
          正在生成答案并逐条核验…
        </div>
      {/if}
      {#if askError}
        <div
          class="ml-10 rounded-lg border border-[#F3BABA] bg-[#FBE6E6] px-3 py-2 text-[12.5px] text-[#991B1B]"
        >
          {askError}
        </div>
      {/if}
    </div>
  </main>

  <!-- 底部:当前依据 + 上传 + 输入 -->
  <div
    class="fixed inset-x-0 bottom-0 z-40 border-t border-[#E7E9EC] bg-[#F7F8FA]/90 px-4 py-3 backdrop-blur"
  >
    <div class="mx-auto max-w-[760px]">
      <div class="mb-1.5 flex flex-wrap items-center gap-1.5 px-1 text-[11.5px] text-[#9AA1A9]">
        <span>当前依据</span>
        {#if uploadedFiles.length}
          {#each uploadedFiles as name (name)}
            <span class="rounded border border-[#E7E9EC] bg-white px-1.5 py-0.5 text-[#5B6370]"
              >📄 {name}</span
            >
          {/each}
        {:else}
          <span class="text-[#9AA1A9]">未选择文档</span>
        {/if}
        {#if currentTokens}
          <span
            class={currentTokens > currentContextLimit
              ? 'font-medium text-[#DC2626]'
              : 'text-[#9AA1A9]'}
            >≈{currentTokens.toLocaleString()} tokens{currentTokens > currentContextLimit
              ? `(超 ${currentContextLimit.toLocaleString()} 上限,可能截断/失败)`
              : ''}</span
          >
        {/if}
        {#if uploading}<span class="text-[#2563EB]">解析中…</span>{/if}
        {#if uploadError}<span class="text-[#DC2626]">{uploadError}</span>{/if}
        {#if ready}
          <span class="ml-auto text-[10px]">实时 · api</span>
        {/if}
      </div>

      <div
        class="flex items-end gap-2 rounded-2xl border border-[#DDE1E6] bg-white p-2 shadow-[0_12px_32px_rgba(16,24,40,0.07)]"
      >
        <input
          bind:this={fileInput}
          type="file"
          multiple
          class="hidden"
          onchange={(e) => {
            const el = e.currentTarget
            if (el.files?.length) onUpload(Array.from(el.files))
            el.value = ''
          }}
        />
        <button
          type="button"
          onclick={() => fileInput?.click()}
          disabled={uploading}
          class="grid h-10 w-10 shrink-0 place-items-center rounded-xl text-[18px] text-[#5B6370] transition hover:bg-[#F1F3F5] disabled:opacity-50"
          aria-label="上传文档"
          title="上传文档(多文件,经 spoor 解析)">📎</button
        >
        <textarea
          bind:value={input}
          rows="1"
          placeholder="{ready ? '输入问题…' : '请先上传文档'}"
          onkeydown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              submitAsk()
            }
          }}
          class="max-h-28 min-h-10 flex-1 resize-none bg-transparent px-2 py-2.5 text-[14px] leading-5 outline-none placeholder:text-[#9AA1A9]"
        ></textarea>
        <button
          type="button"
          onclick={submitAsk}
          disabled={!input.trim() || !ready || asking}
          class="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-[#2563EB] text-[18px] text-white transition hover:bg-[#1D4ED8] disabled:cursor-not-allowed disabled:bg-[#CBD2DA]"
          aria-label="发送">↑</button
        >
      </div>
    </div>
  </div>

  <SourceDrawer
    open={drawer.open}
    markdown={drawer.markdown}
    locate={drawer.locate}
    status={drawer.status}
    title={drawer.title}
    page={drawer.page}
    anchor={drawer.anchor}
    corpusId={drawer.corpusId}
    onClose={() => (drawer = { ...drawer, open: false })}
  />
</div>
