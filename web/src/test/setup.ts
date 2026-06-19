// 全局测试 setup:扩展 expect 加入 toBeInTheDocument / toHaveTextContent 等
// jest-dom matchers (vitest 友好版)。每个测试文件无需重复 import。
import '@testing-library/jest-dom/vitest'

// jsdom 不实现 window.matchMedia —— Modal 用它判断精确指针(桌面)以决定挂载聚焦目标。
// 提供最小桩(matches=false 等价触屏:弹窗仅聚焦容器,不抢焦输入框),避免相关测试在 effect 内崩溃。
if (typeof window !== 'undefined' && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia
}
