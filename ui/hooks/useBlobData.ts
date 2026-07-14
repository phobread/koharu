'use client'

import { keepPreviousData, useQuery } from '@tanstack/react-query'

import { getBlob } from '@/lib/api/default/default'
import { convertToBlob } from '@/lib/io/blobConvert'
import { queryClient } from '@/lib/queryClient'

let blobImageCleanupInstalled = false

function installBlobImageCleanup() {
  if (blobImageCleanupInstalled) return
  blobImageCleanupInstalled = true

  queryClient.getQueryCache().subscribe((event) => {
    if (
      event.type === 'removed' &&
      event.query.queryKey[0] === 'blobImage' &&
      typeof event.query.state.data === 'string'
    ) {
      // Object URLs pin blob memory until explicitly revoked; cache eviction is
      // the single owner-release point, and removed only fires for observer-less entries.
      URL.revokeObjectURL(event.query.state.data)
    }
  })
}

if (typeof window !== 'undefined') installBlobImageCleanup()

const blobQueryOptions = (hash: string) => ({
  queryKey: ['blob', hash] as const,
  queryFn: async () => {
    const blob = await getBlob(hash)
    const buf = await (blob as Blob).arrayBuffer()
    return new Uint8Array(buf)
  },
  staleTime: Infinity,
  gcTime: 10 * 60 * 1000,
  structuralSharing: false as const,
})

/** Fetch blob bytes by hash. Keeps previous data as placeholder while loading. */
export function useBlobData(hash: string | undefined): Uint8Array | undefined {
  const { data } = useQuery({
    ...blobQueryOptions(hash ?? ''),
    enabled: !!hash,
    placeholderData: keepPreviousData,
  })
  return hash ? data : undefined
}

const blobImageQueryOptions = (hash: string) => ({
  queryKey: ['blobImage', hash] as const,
  queryFn: async () => {
    const response = await getBlob(hash)
    const buf = await (response as Blob).arrayBuffer()
    const bytes = new Uint8Array(buf)
    const blob = await convertToBlob(bytes)
    const url = URL.createObjectURL(blob)
    await new Promise<void>((resolve, reject) => {
      const img = new Image()
      img.onload = () => resolve()
      img.onerror = () => reject(new Error('Failed to preload sprite'))
      img.src = url
    })
    return url
  },
  staleTime: Infinity,
  gcTime: 10 * 60 * 1000,
  structuralSharing: false as const,
})

/**
 * Fetch blob, convert to displayable format, and preload — returns a
 * ready-to-paint object URL. Keeps the previous URL while a new one loads.
 */
export function useBlobImage(hash: string | undefined) {
  return useQuery({
    ...blobImageQueryOptions(hash ?? ''),
    enabled: !!hash,
    placeholderData: keepPreviousData,
  })
}
