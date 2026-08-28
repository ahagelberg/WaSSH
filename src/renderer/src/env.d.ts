/// <reference types="vite/client" />

import type { WasshApi } from '../../shared/types'

interface FontData {
  family: string
  fullName: string
  postscriptName: string
  style: string
}

declare global {
  interface Window {
    wassh: WasshApi
    queryLocalFonts?: () => Promise<FontData[]>
  }
}

export {}
