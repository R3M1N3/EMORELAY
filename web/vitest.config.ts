// vitest 独立 config,与 vite.config.ts 隔离避免 build/dev 上下文耦合。
// 仅在 npm test 跑;npm run build 不读这个文件。
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
