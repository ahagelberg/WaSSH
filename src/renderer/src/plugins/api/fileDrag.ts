/** Minimal drag-event shape needed by `isFileDrag` (avoids importing React types here). */
export interface FileDragEventLike {
  dataTransfer: DataTransfer
}

type WebKitItem = DataTransferItem & {
  webkitGetAsEntry?: () => { isDirectory?: boolean } | null
}

/** True when the drag carries OS files. */
export function isFileDrag(e: FileDragEventLike): boolean {
  return Array.from(e.dataTransfer.types).includes('Files')
}

/** Files dropped from the OS; directories are skipped (cannot be chunk-uploaded). */
export function collectDroppedFiles(dt: DataTransfer | null): File[] {
  if (!dt) {
    return []
  }
  const items = dt.items
  if (items && items.length > 0) {
    const files: File[] = []
    for (const item of Array.from(items)) {
      if (item.kind !== 'file') {
        continue
      }
      const entry = (item as WebKitItem).webkitGetAsEntry?.()
      if (entry?.isDirectory) {
        continue
      }
      const file = item.getAsFile()
      if (file) {
        files.push(file)
      }
    }
    return files
  }
  return Array.from(dt.files)
}
