// `npm test`（tsx --test）で実行する。Vue/DOM に依存しない純粋関数のみ検証する。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { ADMIN_LOGIN_PATH, buildLoginUrl, currentReturnTo } from './loginRedirect'

describe('buildLoginUrl', () => {
  it('points at the admin login, not the legacy /login', () => {
    assert.equal(ADMIN_LOGIN_PATH, '/admin/login')
    assert.ok(buildLoginUrl('/todo').startsWith('/admin/login?'))
    assert.ok(!buildLoginUrl('/todo').startsWith('/login'))
  })

  it('carries the current PWA path as an encoded returnTo', () => {
    assert.equal(buildLoginUrl('/todo'), '/admin/login?returnTo=%2Ftodo')
    assert.equal(buildLoginUrl('/'), '/admin/login?returnTo=%2F')
  })

  it('round-trips query strings and hashes through URLSearchParams', () => {
    const target = '/chat?from=chat&x=a b#latest'
    const url = new URL(buildLoginUrl(target), 'http://localhost')
    assert.equal(url.pathname, '/admin/login')
    assert.equal(url.searchParams.get('returnTo'), target)
  })

  it('omits returnTo when there is nothing (or nothing safe) to return to', () => {
    for (const value of [undefined, null, '', 'todo', '//evil.example', '///evil.example/x', '/\\evil.example', 'https://evil.example/', 'javascript:alert(1)']) {
      assert.equal(buildLoginUrl(value), '/admin/login', String(value))
    }
  })
})

describe('currentReturnTo', () => {
  it('joins pathname, search and hash', () => {
    assert.equal(currentReturnTo({ pathname: '/calendar', search: '?month=2026-08', hash: '#d15' }), '/calendar?month=2026-08#d15')
    assert.equal(currentReturnTo({ pathname: '/', search: '', hash: '' }), '/')
  })
})
