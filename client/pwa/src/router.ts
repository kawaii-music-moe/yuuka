import { createRouter, createWebHistory } from 'vue-router'

export default createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes: [
    { path: '/', component: () => import('./pages/DashboardPage.vue'), meta: { title: 'ホーム' } },
    { path: '/todo', component: () => import('./pages/TodoPage.vue'), meta: { title: 'タスク' } },
    { path: '/calendar', component: () => import('./pages/CalendarPage.vue'), meta: { title: 'カレンダー' } },
    { path: '/finance', component: () => import('./pages/FinancePage.vue'), meta: { title: '家計' } },
    { path: '/notes', component: () => import('./pages/NotesPage.vue'), meta: { title: '共有ノート' } },
    { path: '/chat', component: () => import('./pages/ChatPage.vue'), meta: { title: 'チャット' } },
    { path: '/settings', component: () => import('./pages/SettingsPage.vue'), meta: { title: '設定' } },
    { path: '/management-vue', component: () => import('./features/admin/pages/AdminBotSelectionPage.vue'), meta: { title: 'Bot一覧' } },
    {
      path: '/:pathMatch(.*)*',
      component: () => import('./pages/NotFoundPage.vue'),
      meta: { public: true, title: '404' },
    },
  ],
})
