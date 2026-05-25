import { create } from 'zustand';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { DownloadTask, DownloadTaskSchema, ChatMetadata, UnifiedMetadata } from '../types';

// ---------------------------------------------------------------------------
// State shape
// ---------------------------------------------------------------------------
interface DownloadState {
  tasks: Record<string, DownloadTask>;
  isInitialized: boolean;
  error: string | null;
  analyzedMetadata: UnifiedMetadata | null;
  isAnalyzing: boolean;
}

interface DownloadActions {
  init: () => Promise<UnlistenFn>;
  analyzeUrl: (url: string) => Promise<UnifiedMetadata | null>;
  queueDownload: (meta: ChatMetadata, title: string) => Promise<string>;
  clearError: () => void;
  clearAnalyzedMetadata: () => void;
}

type DownloadStore = DownloadState & DownloadActions;

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------
export const useDownloadStore = create<DownloadStore>((set) => ({
  tasks: {},
  isInitialized: false,
  error: null,
  analyzedMetadata: null,
  isAnalyzing: false,

  // -------------------------------------------------------------------------
  // init: hydrate from backend + subscribe to live events
  // Returns the unlisten fn so callers can clean up on unmount.
  // -------------------------------------------------------------------------
  init: async () => {
    // 1. Hydrate existing tasks from backend
    try {
      const raw = await invoke<unknown[]>('get_download_queue');
      const tasks: Record<string, DownloadTask> = {};
      for (const item of raw) {
        const parsed = DownloadTaskSchema.safeParse(item);
        if (parsed.success) {
          tasks[parsed.data.taskId] = parsed.data;
        }
      }
      set({ tasks, isInitialized: true });
    } catch (err) {
      console.error('[DownloadStore] Failed to hydrate queue:', err);
      set({ error: String(err), isInitialized: true });
    }

    // 2. Subscribe to real-time progress events from Tauri backend
    const unlisten = await listen<unknown>('download-progress', (event) => {
      const parsed = DownloadTaskSchema.safeParse(event.payload);
      if (!parsed.success) {
        console.warn('[DownloadStore] Received invalid payload:', event.payload);
        return;
      }
      const task = parsed.data;
      set((state) => ({
        tasks: { ...state.tasks, [task.taskId]: task },
      }));
    });

    return unlisten;
  },

  // -------------------------------------------------------------------------
  // analyzeUrl: calls analyze_stream_url on the backend to get metadata
  // -------------------------------------------------------------------------
  analyzeUrl: async (url) => {
    set({ isAnalyzing: true, error: null });
    try {
      const metadata = await invoke<UnifiedMetadata | null>('analyze_stream_url', { url });
      set({ analyzedMetadata: metadata, isAnalyzing: false });
      return metadata;
    } catch (err) {
      const msg = String(err);
      set({ error: msg, isAnalyzing: false });
      throw new Error(msg);
    }
  },

  // -------------------------------------------------------------------------
  // queueDownload: sends ChatMetadata to the backend, returns the task ID
  // -------------------------------------------------------------------------
  queueDownload: async (meta, title) => {
    try {
      const taskId = await invoke<string>('queue_chat_download', { meta, title });
      return taskId;
    } catch (err) {
      const msg = String(err);
      set({ error: msg });
      throw new Error(msg);
    }
  },

  clearError: () => set({ error: null }),
  clearAnalyzedMetadata: () => set({ analyzedMetadata: null }),
}));
