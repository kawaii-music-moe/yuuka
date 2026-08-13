<script setup lang="ts">
import { computed } from 'vue'
import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'
import footnote from 'markdown-it-footnote'
import taskLists from 'markdown-it-task-lists'
import hljs from 'highlight.js/lib/common'
import 'highlight.js/styles/github.css'

const props = defineProps<{ source: string }>()
const escapeHtml = (value: string) => value.replace(/[&<>"]/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[char] ?? char)
const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true, highlight: (code: string, language: string) => {
  if (language && hljs.getLanguage(language)) return `<pre class="hljs"><code>${hljs.highlight(code, { language }).value}</code></pre>`
  return `<pre class="hljs"><code>${escapeHtml(code)}</code></pre>`
} }).use(taskLists, { enabled: true, label: true }).use(footnote)
const rendered = computed(() => DOMPurify.sanitize(markdown.render(props.source), { USE_PROFILES: { html: true } }))
</script>
<template><div class="markdown-content" v-html="rendered"></div></template>
<style>.markdown-content{font-size:14px;line-height:1.65;overflow-wrap:anywhere}.markdown-content>:first-child{margin-top:0}.markdown-content>:last-child{margin-bottom:0}.markdown-content h1,.markdown-content h2,.markdown-content h3{line-height:1.35;margin:1em 0 .45em}.markdown-content h1{font-size:1.35em}.markdown-content h2{font-size:1.2em}.markdown-content h3{font-size:1.05em}.markdown-content p{margin:.55em 0}.markdown-content ul,.markdown-content ol{padding-left:1.4em;margin:.55em 0}.markdown-content blockquote{border-left:3px solid var(--text-medium);margin:.7em 0;padding-left:.75em;color:var(--text-medium)}.markdown-content code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;background:var(--surface-3);padding:1px 4px;border-radius:2px;font-size:.9em}.markdown-content pre{overflow:auto;border:1px solid var(--line);padding:10px;background:var(--surface-2);border-radius:2px}.markdown-content pre code{padding:0;background:transparent}.markdown-content table{border-collapse:collapse;display:block;overflow:auto;margin:.75em 0}.markdown-content th,.markdown-content td{border:1px solid var(--line);padding:5px 8px;text-align:left}.markdown-content th{background:var(--surface-3)}.markdown-content a{color:var(--primary);text-decoration:underline}.markdown-content img{max-width:100%;height:auto}.markdown-content input[type=checkbox]{accent-color:var(--primary)}</style>
