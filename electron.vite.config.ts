import { resolve } from 'path'
import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()],
    resolve: {
      alias: {
        '@shared': resolve('src/shared'),
        '@plugin-api/shared': resolve('src/shared/pluginApi.ts'),
        '@plugin-api/main': resolve('src/main/plugins/api.ts')
      }
    }
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    resolve: {
      alias: {
        '@shared': resolve('src/shared'),
        '@plugin-api/shared': resolve('src/shared/pluginApi.ts')
      }
    }
  },
  renderer: {
    resolve: {
      alias: {
        '@renderer': resolve('src/renderer/src'),
        '@shared': resolve('src/shared'),
        '@plugin-api/shared': resolve('src/shared/pluginApi.ts'),
        '@plugin-api/renderer': resolve('src/renderer/src/plugins/api/index.ts')
      }
    },
    plugins: [react()]
  }
})
