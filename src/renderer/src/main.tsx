import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import './styles/app.css'
import './styles/settings.css'
import './styles/plugins.css'
import '@xterm/xterm/css/xterm.css'
import '@fontsource/jetbrains-mono/latin-400.css'
import '@fontsource/jetbrains-mono/latin-700.css'
import '@fontsource/ibm-plex-mono/latin-400.css'
import '@fontsource/ibm-plex-mono/latin-700.css'

const root = document.getElementById('root')
if (!root) {
  throw new Error('root element missing')
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>
)
