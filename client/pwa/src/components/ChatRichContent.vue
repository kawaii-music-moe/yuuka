<script setup lang="ts">
import type { ChatEmbed, ChatFile } from '@/api'
import AppIcon from './AppIcon.vue'
import { mediaKind } from '@/utils/chatAttachments'
import MarkdownContent from './MarkdownContent.vue'

/**
 * メッセージのリッチコンテンツ（Discord Embed 相当の埋め込みとファイル添付）。
 * embed の色は 0xRRGGBB の数値で届く。ファイルの実体は認証必須の同一オリジン URL
 * （所有者スコープ）なので、画像は `<img>`、音声・動画はその場で再生、それ以外はダウンロードリンクにする。
 */
defineProps<{ embeds?: ChatEmbed[]; files?: ChatFile[] }>()
const accent = (color?: number) => (color === undefined ? undefined : `#${(color & 0xffffff).toString(16).padStart(6, '0')}`)
const kind = (file: ChatFile) => mediaKind(file.mimeType)
</script>
<template>
  <div v-if="embeds?.length || files?.length" class="rich-content">
    <article v-for="(embed, index) in embeds" :key="`embed-${index}`" class="embed" :style="accent(embed.color) ? { borderLeftColor: accent(embed.color) } : undefined">
      <strong v-if="embed.title" class="embed-title">{{ embed.title }}</strong>
      <MarkdownContent v-if="embed.description" class="embed-description" :source="embed.description" />
      <dl v-if="embed.fields?.length" class="embed-fields">
        <div v-for="(field, fieldIndex) in embed.fields" :key="`field-${fieldIndex}`" class="embed-field" :class="{ inline: field.inline }">
          <dt>{{ field.name }}</dt>
          <dd><MarkdownContent :source="field.value" /></dd>
        </div>
      </dl>
      <small v-if="embed.footer" class="embed-footer">{{ embed.footer }}</small>
    </article>
    <ul v-if="files?.length" class="files">
      <li v-for="file in files" :key="file.id" :class="kind(file)">
        <a v-if="kind(file) === 'image'" :href="file.url" target="_blank" rel="noopener" :title="file.name"><img :src="file.url" :alt="file.name" loading="lazy" /></a>
        <video v-else-if="kind(file) === 'video'" :src="file.url" controls preload="metadata" :title="file.name"></video>
        <audio v-else-if="kind(file) === 'audio'" :src="file.url" controls preload="metadata" :title="file.name"></audio>
        <a v-else :href="file.url" :download="file.name" class="file-link"><AppIcon name="download" /><span>{{ file.name }}</span></a>
      </li>
    </ul>
  </div>
</template>
<style scoped>
.rich-content { display: grid; gap: 8px; margin-top: 8px; }
.embed { border: 1px solid var(--line); border-left: 4px solid var(--primary); background: var(--surface-2, rgba(255, 255, 255, .04)); border-radius: 2px; padding: 9px 12px; display: grid; gap: 6px; min-width: 0; }
.embed-title { font-size: 14px; }
.embed-description { font-size: 13px; }
.embed-fields { margin: 0; display: flex; flex-wrap: wrap; gap: 8px 16px; }
.embed-field { flex: 1 1 100%; min-width: 0; }
.embed-field.inline { flex: 1 1 130px; }
.embed-field dt { font-size: 12px; font-weight: 700; color: var(--text-medium); margin-bottom: 2px; }
.embed-field dd { margin: 0; }
.embed-field dd :deep(.markdown-content) { font-size: 13px; }
.embed-footer { font-size: 11px; color: var(--text-medium); }
.files { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 8px; }
.files img { display: block; max-width: 100%; max-height: 320px; border: 1px solid var(--line); border-radius: 2px; background: #fff; }
.files li.image, .files li.video { max-width: 100%; }
.files video { display: block; max-width: 100%; max-height: 320px; border-radius: 2px; background: #000; }
.files audio { display: block; width: min(320px, 100%); }
.file-link { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--primary-border); background: var(--primary-soft); padding: 6px 10px; border-radius: 2px; font-size: 13px; color: var(--primary); }
</style>
