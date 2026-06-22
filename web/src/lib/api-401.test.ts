// A1 回归:会话中途 401(token 过期 / 被吊销 / 另端登出)时,api 层应清 token 并广播
// `emorelay:unauthorized` 全局事件,供 AuthProvider 登出 + 提示 + 跳登录(修复「静默失效」)。
import { describe, it, expect, vi, afterEach } from 'vitest'
import { api, getToken, setToken, clearToken } from './api'

afterEach(() => {
  clearToken()
  vi.restoreAllMocks()
})

function mockFetch(status: number, bodyObj: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue({
      status,
      ok: status >= 200 && status < 300,
      text: async () => JSON.stringify(bodyObj),
    }),
  )
}

describe('api 401 会话失效广播', () => {
  it('401 时清 token 并派发 emorelay:unauthorized 事件', async () => {
    setToken('stale-token')
    mockFetch(401, { error: 'unauthorized', message: '未授权' })
    let fired = 0
    const h = () => {
      fired++
    }
    window.addEventListener('emorelay:unauthorized', h)
    await expect(api.get('/api/anything')).rejects.toMatchObject({ status: 401 })
    window.removeEventListener('emorelay:unauthorized', h)
    expect(fired).toBe(1)
    expect(getToken()).toBeNull()
  })

  it('非 401 错误不派发事件、不清 token', async () => {
    setToken('good-token')
    mockFetch(500, { error: 'server_error', message: '服务器错误' })
    let fired = 0
    const h = () => {
      fired++
    }
    window.addEventListener('emorelay:unauthorized', h)
    await expect(api.get('/api/anything')).rejects.toMatchObject({ status: 500 })
    window.removeEventListener('emorelay:unauthorized', h)
    expect(fired).toBe(0)
    expect(getToken()).toBe('good-token')
  })
})
