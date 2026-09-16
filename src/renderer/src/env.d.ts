/// <reference types="vite/client" />

interface FontData {
  family: string
  fullName: string
  postscriptName: string
  style: string
}

declare global {
  interface Window {
    queryLocalFonts?: () => Promise<FontData[]>
  }
}

export {}
