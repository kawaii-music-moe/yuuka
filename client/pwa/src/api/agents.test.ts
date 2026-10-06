// `npm test`（tsx --test）で実行する。Node には localStorage が無いため、選択の保存は黙って
// スキップされる（このタブの中だけで切り替わる）経路を通る。
import assert from 'node:assert/strict'
import { afterEach, describe, it } from 'node:test'
import { DEFAULT_AGENT_ID, selectAgent, selectedAgentId, withSelectedAgent } from './agents'

describe('withSelectedAgent', () => {
  afterEach(() => selectAgent(DEFAULT_AGENT_ID))

  it('starts with the secretary and leaves paths unchanged for it', () => {
    assert.equal(selectedAgentId.value, DEFAULT_AGENT_ID)
    assert.equal(withSelectedAgent('/api/client/todos'), '/api/client/todos')
  })

  it('adds botId for another agent', () => {
    selectAgent('bot_123')
    assert.equal(selectedAgentId.value, 'bot_123')
    assert.equal(withSelectedAgent('/api/client/todos'), '/api/client/todos?botId=bot_123')
  })

  it('appends to an existing query and encodes the id', () => {
    selectAgent('bot a&b')
    assert.equal(
      withSelectedAgent('/api/client/finance/summary?month=2026-08'),
      '/api/client/finance/summary?month=2026-08&botId=bot%20a%26b',
    )
  })
})
