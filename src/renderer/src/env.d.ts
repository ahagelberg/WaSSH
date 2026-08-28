/// <reference types="vite/client" />

import type { WasshApi } from '../../shared/types'

declare global {
  interface Window {
    wassh: WasshApi
  }
}

export {}
