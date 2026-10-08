// `npm test`（tsx --test）で実行する。Node には localStorage が無いため、選択の保存は黙って
// スキップされる（このタブの中だけで切り替わる）経路を通る。
import assert from 'node:assert/strict'
import { afterEach, describe, it } from 'node:test'
import { SYSTEM_AGENT_ID, selectAgent, selectedAgentId, toAgents, withSelectedAgent } from './agents'

describe('withSelectedAgent', () => {
  afterEach(() => selectAgent(''))

  it('starts with no agent selected', () => {
    assert.equal(selectedAgentId.value, '')
  })

  it('adds botId for the selected agent', () => {
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

describe('toAgents', () => {
  it('leaves out the system bot and keeps the server order', () => {
    assert.deepEqual(
      toAgents([
        { id: SYSTEM_AGENT_ID, name: 'システムデフォルト' },
        { id: 'bot_2', name: 'shared' },
        { id: 'bot_1', name: 'mine' },
      ]),
      [
        { id: 'bot_2', name: 'shared' },
        { id: 'bot_1', name: 'mine' },
      ],
    )
  })

  it('returns an empty list when only the system bot exists', () => {
    assert.deepEqual(toAgents([{ id: SYSTEM_AGENT_ID, name: 'システムデフォルト' }]), [])
  })
})
