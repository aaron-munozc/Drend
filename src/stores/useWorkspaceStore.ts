/**
 * useWorkspaceStore.ts
 * Zustand stores that fully replaces WorkspaceContext.
 * Persists tabs + task-snapshots via zustand/middleware "persist" backed by
 * localStorage (Tauri's window.localStorage is available in WebView2 / WKWebView).
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";
import {
    FrontendChatOptions,
    FrontendVodOptions,
    Metadata,
    RENDER_DEFAULTS,
    RenderVideoArgs,
} from "@/types/backend.ts";

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

export type DownloadMode = "vod" | "chat";

export interface TabState {
    id: string;
    label: string;
    url: string;
    metadata: Metadata | null;
    isAnalyzing: boolean;
    analyzeError: string | null;
    downloadMode: DownloadMode;
    vodOptions: FrontendVodOptions;
    chatOptions: FrontendChatOptions;
    jsonFilePath: string;
    renderOptions: Partial<RenderVideoArgs>;
    activeTaskId: string | null;
}

export interface TabSnapshot {
    tabId: string;
    taskId: string;
    url?: string;
    jsonFilePath?: string;
    vodOptions?: FrontendVodOptions;
    chatOptions?: FrontendChatOptions;
    renderOptions?: Partial<RenderVideoArgs>;
    downloadMode?: DownloadMode;
    metadata?: Metadata | null;
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

let _tabCounter = 1;

function makeDefaultTab(
    id: string,
    index: number,
    initialData?: Partial<TabState>
): TabState {
    return {
        id,
        label: `Workspace ${index}`,
        url: "",
        metadata: null,
        isAnalyzing: false,
        analyzeError: null,
        downloadMode: "vod",
        vodOptions: {},
        chatOptions: {},
        jsonFilePath: "",
        renderOptions: { ...RENDER_DEFAULTS },
        activeTaskId: null,
        ...initialData,
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Store shape
// ─────────────────────────────────────────────────────────────────────────────

interface WorkspaceState {
    tabs: TabState[];
    activeTabId: string | null;
    /** Serialisable snapshot map — stored as a plain object for persistence */
    taskSnapshots: Record<string, TabSnapshot>;

    // Tab management
    setActiveTab: (id: string) => void;
    addTab: (initialData?: Partial<TabState>) => string;
    closeTab: (id: string) => void;
    renameTab: (id: string, label: string) => void;
    updateTab: (id: string, patch: Partial<TabState>) => void;
    getTab: (id: string) => TabState | undefined;

    // Snapshot management
    registerTaskSnapshot: (snapshot: TabSnapshot) => void;
    getSnapshotByTaskId: (taskId: string) => TabSnapshot | undefined;
}

// ─────────────────────────────────────────────────────────────────────────────
// Store
// ─────────────────────────────────────────────────────────────────────────────

export const useWorkspaceStore = create<WorkspaceState>()(
    persist(
        immer<WorkspaceState>((set, get) => ({
            tabs: (() => {
                const id = crypto.randomUUID();
                return [makeDefaultTab(id, _tabCounter)];
            })(),
            activeTabId: null,
            taskSnapshots: {},

            setActiveTab: (id) =>
                set((s) => {
                    s.activeTabId = id;
                }),

            addTab: (initialData) => {
                _tabCounter += 1;
                const id = crypto.randomUUID();
                const newTab = makeDefaultTab(id, _tabCounter, initialData);
                set((s) => {
                    s.tabs.push(newTab);
                    s.activeTabId = id;
                });
                return id;
            },

            closeTab: (id) =>
                set((s) => {
                    const idx = s.tabs.findIndex((t: TabState) => t.id === id);
                    s.tabs = s.tabs.filter((t: TabState) => t.id !== id);
                    if (s.activeTabId === id) {
                        s.activeTabId = s.tabs[Math.max(0, idx - 1)]?.id ?? null;
                    }
                }),

            renameTab: (id, label) =>
                set((s) => {
                    const t = s.tabs.find((t: TabState) => t.id === id);
                    if (t) t.label = label;
                }),

            updateTab: (id, patch) =>
                set((s) => {
                    const t = s.tabs.find((t: TabState) => t.id === id);
                    if (t) Object.assign(t, patch);
                }),

            getTab: (id) => get().tabs.find((t) => t.id === id),

            registerTaskSnapshot: (snapshot) =>
                set((s) => {
                    s.taskSnapshots[snapshot.taskId] = snapshot;
                }),

            getSnapshotByTaskId: (taskId) => get().taskSnapshots[taskId],
        })),
        {
            name: "workspace-storage",
            storage: createJSONStorage(() => localStorage),
            // Restore tabCounter from persisted tabs so new tabs don't collide
            onRehydrateStorage: () => (state) => {
                if (state) {
                    _tabCounter = Math.max(
                        _tabCounter,
                        state.tabs.length > 0 ? state.tabs.length : 1
                    );
                    // Guarantee there's always an active tab after reload
                    if (!state.activeTabId && state.tabs.length > 0) {
                        const firstTab = state.tabs[0];
                        if (firstTab) {
                            state.activeTabId = firstTab.id;
                        }
                    }
                }
            },
            // Don't persist transient UI flags
            partialize: (state) => ({
                tabs: state.tabs.map((t) => ({
                    ...t,
                    isAnalyzing: false,
                    analyzeError: null,
                })),
                activeTabId: state.activeTabId,
                taskSnapshots: state.taskSnapshots,
            }),
        }
    )
);

// ─────────────────────────────────────────────────────────────────────────────
// Selector hooks (memoised slices to avoid full re-renders)
// ─────────────────────────────────────────────────────────────────────────────

export const useTabs = () => useWorkspaceStore((s) => s.tabs);
export const useActiveTabId = () => useWorkspaceStore((s) => s.activeTabId);
export const useActiveTab = () =>
    useWorkspaceStore((s) => s.tabs.find((t) => t.id === s.activeTabId) ?? null);

/** Shim that mirrors the old useWorkspace() hook signature so existing call sites can
 *  migrate with a one-line import swap. */
export function useWorkspace() {
    return useWorkspaceStore();
}