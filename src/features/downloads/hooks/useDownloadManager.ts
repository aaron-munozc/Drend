import { useEffect, useRef } from 'react';
import { type UnlistenFn } from '@tauri-apps/api/event';
import { useDownloadStore } from '../store/downloadStore';

/**
 * Initializes the download store once and cleans up the Tauri event listener
 * on unmount. Safe to call from multiple components — Zustand's isInitialized
 * flag ensures the backend call only happens once.
 */
export function useDownloadManager() {
  const init = useDownloadStore((s) => s.init);
  const isInitialized = useDownloadStore((s) => s.isInitialized);
  const unlistenRef = useRef<UnlistenFn | null>(null);

  useEffect(() => {
    if (isInitialized) return;

    let cancelled = false;
    init().then((unlisten) => {
      if (cancelled) {
        unlisten();
        return;
      }
      unlistenRef.current = unlisten;
    });

    return () => {
      cancelled = true;
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [init, isInitialized]);
}
