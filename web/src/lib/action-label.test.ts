import { describe, expect, it } from 'vitest'
import { actionLabel } from './api'

describe('actionLabel', () => {
  it('命中已知 action 返回中文别名', () => {
    expect(actionLabel('auth.login')).toBe('用户登录')
    expect(actionLabel('rule.create')).toBe('创建规则')
    expect(actionLabel('node.delete')).toBe('删除节点')
    expect(actionLabel('tunnel.creds_rotated')).toBe('隧道凭据轮换')
    expect(actionLabel('system.update_settings')).toBe('修改系统设置')
  })
  it('未知 action 兜底返回原值(新增 action 不会显示空白)', () => {
    expect(actionLabel('foo.bar')).toBe('foo.bar')
    expect(actionLabel('')).toBe('')
  })
})
