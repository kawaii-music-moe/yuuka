<template>
  <main class="not-found-page" aria-labelledby="not-found-title">
    <p class="not-found-code">404</p>
    <h1 id="not-found-title">ページが見つかりません</h1>
    <p class="not-found-copy">指定された Client のページは存在しないか、移動されています。</p>
    <RouterLink class="not-found-action" to="/">ホームへ戻る</RouterLink>
  </main>
</template>

<style scoped>
.not-found-page {
  min-height: 100dvh;
  display: grid;
  align-content: center;
  justify-items: start;
  gap: 16px;
  padding: 32px 24px;
  background: #121212;
  color: #f4f4f5;
}

.not-found-code {
  margin: 0;
  color: #60a5fa;
  font-size: 14px;
  font-weight: 700;
  letter-spacing: 0.12em;
}

h1 { margin: 0; font-size: clamp(28px, 8vw, 40px); }
.not-found-copy { margin: 0; color: #a1a1aa; line-height: 1.65; }
.not-found-action {
  color: #dbeafe;
  background: #2563eb;
  border-radius: 4px;
  padding: 10px 14px;
  font-weight: 700;
  text-decoration: none;
}
</style>
