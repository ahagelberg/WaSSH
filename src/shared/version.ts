import packageJson from '../../package.json'

/**
 * App version — single source of truth is package.json "version".
 * Bump there for releases; electron-builder and the About dialog both read it.
 */
export const APP_VERSION: string = packageJson.version

export const APP_NAME = 'WaSSH'

export const APP_DESCRIPTION: string = packageJson.description

export const APP_LICENSE: string = packageJson.license

export const APP_AUTHOR: string = packageJson.author
