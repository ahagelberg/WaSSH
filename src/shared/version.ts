import packageJson from '../../package.json'

/**
 * App version — single source of truth is package.json "version".
 * Bump there for releases; electron-builder and the About dialog both read it.
 */
export const APP_VERSION: string = packageJson.version

export const APP_NAME = 'WaSSH'

/** Windows Application User Model ID — must match package.json build.appId (NSIS shortcuts). */
export const APP_ID: string = packageJson.build.appId

export const APP_DESCRIPTION: string = packageJson.description

export const APP_LICENSE: string = packageJson.license

export const APP_AUTHOR: string = packageJson.author
