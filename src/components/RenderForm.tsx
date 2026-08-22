
/**
 * RenderForm.tsx
 */

import { useNavigate } from "@tanstack/react-router";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import React, {
    memo,
    useCallback,
    useEffect,
    useMemo,
    useRef,
    useState,
} from "react";
import { TabState, useWorkspace } from "@/stores/useWorkspaceStore.ts";
import type {
    BackgroundMode,
    CustomImageOverlay,
    CustomShapeOverlay,
    EvictionStrategy,
    QualityPreset,
    RenderVideoArgs,
    TimelineMismatchStrategy,
} from "../types/backend";
import { RENDER_DEFAULTS } from "../types/backend";
import { ColorPicker } from "./ui/ColorPicker";
import { CustomSlider } from "./ui/CustomSlider";

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

interface RenderFormProps {
    tab: TabState;
    onUpdate: (patch: Partial<TabState>) => void;
}

interface VideoMeta {
    width: number;
    height: number;
    duration: number;
    /** Tauri asset URL for <video> playback */
    videoSrc: string;
    /** First-frame JPEG data-url for initial display before video loads */
    posterDataUrl: string;
}

type OverlayKind = "chat" | "shape" | "image";
type EditMode = "video" | "chat" | "shapes";

interface DragState {
    kind: OverlayKind | "scrub";
    index?: number;
    /** null = move, else edge name */
    edge: "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw" | null;
    startX: number;
    startY: number;
    initRect: { x: number; y: number; width: number; height: number };
    initTime?: number;
    initBgW: number;
    initBgH: number;
    initScreenW: number;
    initScreenH: number;
}

const BATCH_ITEM_EXCLUSIVE_KEYS = new Set<keyof RenderVideoArgs>([
    // Source video and output destination are intrinsically item-specific.
    // Direct Pipe mode + overlay geometry intentionally inherit from the template.
    "outputPath",
    "overlayVideoPath",
]);

interface BatchItem {
    id: string;
    jsonFilePath: string;
    outputPath: string;
    outputPathOverridden?: boolean;
    /** Base video stays item-specific. The Direct Pipe toggle inherits through overrides. */
    overlayVideoPath: string;
    overrides: Partial<Omit<RenderVideoArgs, "outputPath" | "overlayVideoPath">>;
}

type BatchMatchMode = "glob" | "regex";

interface BatchPair {
    key: string;
    jsonFilePath: string;
    /** Optional item-specific base video. Required only when that item uses Direct Pipe mode. */
    videoFilePath?: string;
}

function makeBatchItem(): BatchItem {
    return {
        id: crypto.randomUUID(),
        jsonFilePath: "",
        outputPath: "",
        outputPathOverridden: false,
        overlayVideoPath: "",
        overrides: {},
    };
}

function getPathFileName(path: string) {
    return path.split(/[/\\]/).pop() ?? path;
}

function getPathStem(path: string) {
    return getPathFileName(path).replace(/\.[^.]+$/, "");
}

function getPathDirectory(path: string) {
    const match = path.match(/^(.*)[/\\][^/\\]+$/);
    return match?.[1] ?? "";
}

function joinPath(directory: string, fileName: string) {
    if (!directory) return fileName;
    const sep = directory.includes("\\") && !directory.includes("/") ? "\\" : "/";
    return directory.endsWith(sep) ? `${directory}${fileName}` : `${directory}${sep}${fileName}`;
}

function globToRegExp(glob: string) {
    const escaped = glob.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`^${escaped.replace(/\*/g, ".*").replace(/\?/g, ".")}$`, "i");
}

function matchesBatchPattern(stem: string, pattern: string, mode: BatchMatchMode) {
    if (!pattern.trim()) return true;
    try {
        return mode === "regex" ? new RegExp(pattern, "i").test(stem) : globToRegExp(pattern).test(stem);
    } catch {
        return false;
    }
}

function pairBatchFiles(paths: string[], pattern: string, mode: BatchMatchMode): BatchPair[] {
    const jsonByStem = new Map<string, string>();
    const videoByStem = new Map<string, string>();

    for (const path of paths) {
        const ext = getPathFileName(path).split(".").pop()?.toLowerCase();
        const stem = getPathStem(path);
        if (!stem || !matchesBatchPattern(stem, pattern, mode)) continue;

        const key = stem.toLowerCase();
        if (ext === "jsonl" && !jsonByStem.has(key)) jsonByStem.set(key, path);
        if (["mp4", "mkv", "mov", "avi", "webm"].includes(ext ?? "") && !videoByStem.has(key)) {
            videoByStem.set(key, path);
        }
    }

    return Array.from(jsonByStem.entries())
        .sort(([a], [b]) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" }))
        .map(([key, jsonFilePath]) => ({
            key,
            jsonFilePath,
            videoFilePath: videoByStem.get(key),
        }));
}

function deriveBatchOutputPath(directory: string, jsonPath: string) {
    if (!jsonPath) return "";
    const stem = getPathStem(jsonPath);
    return joinPath(directory || getPathDirectory(jsonPath), `${stem}.rendered.mp4`);
}

function resolveItemOpts(main: RenderVideoArgs, item: BatchItem): RenderVideoArgs {
    return {
        ...main,
        ...item.overrides,
        outputPath: item.outputPath,
        overlayVideoPath: item.overlayVideoPath || undefined,
    };
}

function extractCopyableSettings(opts: RenderVideoArgs) {
    const result: Partial<RenderVideoArgs> = { ...opts };
    for (const key of BATCH_ITEM_EXCLUSIVE_KEYS) {
        Reflect.deleteProperty(result, key);
    }
    return result;
}

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const BG_MODES: { value: BackgroundMode; label: string }[] = [
    { value: "transparent", label: "Transparent" },
    { value: "lumaMatte", label: "Luma Matte" },
    { value: "chromaKeyGreen", label: "Chroma Key" },
    { value: "customColor", label: "Custom Color" },
];

const EVICTION_MODES: { value: EvictionStrategy; label: string; description: string }[] = [
    { value: "pushOnly", label: "Push Only", description: "Messages stay until pushed off-screen" },
    { value: "timed", label: "Timed", description: "Messages fade out after a set duration" },
];

const QUALITY_PRESETS: { value: QualityPreset; label: string; description: string }[] = [
    { value: "draft", label: "Draft", description: "Fastest · nearest-neighbor" },
    { value: "standard", label: "Standard", description: "Bilinear · balanced" },
    { value: "high", label: "High", description: "Lanczos3 · best quality" },
];

const TIMELINE_STRATEGIES: { value: TimelineMismatchStrategy; label: string; description: string }[] = [
    { value: "freezeLastFrame", label: "Freeze", description: "Hold last chat frame over remaining video" },
    { value: "renderClearCanvas", label: "Clear", description: "Pass remaining video frames through clean" },
    { value: "loopChatLog", label: "Loop", description: "Restart the chat overlay from the beginning" },
];

const EDGES = ["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const;

// ─────────────────────────────────────────────────────────────────────────────
// OverlayHandle — resize handle for canvas overlays.
// Defined outside CanvasEditor so React sees a stable component type across
// renders; defining it inside (even via useCallback) causes unmount/remount.
// ─────────────────────────────────────────────────────────────────────────────

interface OverlayHandleProps {
    edge: DragState["edge"];
    rect: { x: number; y: number; width: number; height: number };
    kind: OverlayKind;
    index?: number;
    onPointerDown: (
        e: React.PointerEvent,
        kind: OverlayKind,
        index: number | undefined,
        rect: { x: number; y: number; width: number; height: number },
        edge: DragState["edge"],
    ) => void;
}

const OverlayHandle = memo(function OverlayHandle({ edge, rect, kind, index, onPointerDown }: OverlayHandleProps) {
    const pos: React.CSSProperties = {
        position: "absolute", width: 10, height: 10, borderRadius: 3,
        background: "white", border: "2px solid rgba(99,102,241,0.95)", zIndex: 40,
        userSelect: "none", WebkitUserSelect: "none", pointerEvents: "auto",
        touchAction: "none", boxSizing: "border-box",
        cursor: edge ? EDGE_CURSOR[edge] : undefined,
    };
    if (edge === "n") Object.assign(pos, { top: -5, left: "50%", transform: "translateX(-50%)" });
    if (edge === "s") Object.assign(pos, { bottom: -5, left: "50%", transform: "translateX(-50%)" });
    if (edge === "e") Object.assign(pos, { right: -5, top: "50%", transform: "translateY(-50%)" });
    if (edge === "w") Object.assign(pos, { left: -5, top: "50%", transform: "translateY(-50%)" });
    if (edge === "ne") Object.assign(pos, { top: -5, right: -5 });
    if (edge === "nw") Object.assign(pos, { top: -5, left: -5 });
    if (edge === "se") Object.assign(pos, { bottom: -5, right: -5 });
    if (edge === "sw") Object.assign(pos, { bottom: -5, left: -5 });

    return (
        <div
            style={pos}
            onPointerDown={(e) => {
                e.preventDefault();
                e.stopPropagation();
                (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                onPointerDown(e, kind, index, rect, edge);
            }}
        />
    );
});

// ─────────────────────────────────────────────────────────────────────────────
// Primitive UI helpers — all memo'd so parent re-renders don't cascade
// ─────────────────────────────────────────────────────────────────────────────

const FieldLabel = memo(function FieldLabel({ children }: { children: React.ReactNode }) {
    return (
        <label className="block text-[10px] font-semibold text-neutral-500 mb-1 uppercase tracking-wide">
            {children}
        </label>
    );
});

const Divider = memo(function Divider() {
    return <div className="border-b border-neutral-800/80 my-0.5" />;
});

const TextInput = memo(function TextInput({ value, onChange, onBlur, placeholder, mono, disabled }: {
    value: string; onChange: (v: string) => void; onBlur?: () => void;
    placeholder?: string; mono?: boolean; disabled?: boolean;
}) {
    return (
        <input type="text" value={value} placeholder={placeholder} disabled={disabled}
               onChange={(e) => onChange(e.target.value)} onBlur={onBlur}
               className={`w-full bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all disabled:opacity-40 disabled:cursor-not-allowed ${mono ? "font-mono" : ""}`}
        />
    );
});

const NumberInput = memo(function NumberInput({ value, onChange, min, max, step }: {
    value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number;
}) {
    return (
        <input type="number" value={value} min={min} max={max} step={step}
               onChange={(e) => {
                   const v = Number(e.target.value);
                   if (!Number.isNaN(v)) onChange(v);
               }}
               className="w-full bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all"
        />
    );
});

const Toggle = memo(function Toggle({ value, onChange, label, description }: {
    value: boolean; onChange: (v: boolean) => void; label: string; description?: string;
}) {
    return (
        <button type="button" onClick={() => onChange(!value)} className="flex items-start gap-2.5 w-full text-left group">
            <div className={`mt-0.5 relative w-7 h-4 rounded-full shrink-0 transition-colors duration-200 ${value ? "bg-indigo-600" : "bg-neutral-700"}`}>
                <div className={`absolute top-0.5 w-3 h-3 rounded-full bg-white shadow-sm transition-transform duration-200 ${value ? "translate-x-3.5" : "translate-x-0.5"}`} />
            </div>
            <div>
                <div className="text-xs text-neutral-300 font-medium leading-tight">{label}</div>
                {description && <div className="text-[10px] text-neutral-600 mt-0.5">{description}</div>}
            </div>
        </button>
    );
});

const BrowseInput = memo(function BrowseInput({ value, onChange, onBrowse, onClear, placeholder, browseLabel }: {
    value: string; onChange: (v: string) => void; onBrowse: () => void; onClear?: () => void;
    placeholder: string; browseLabel: string;
}) {
    return (
        <div className="flex gap-1.5">
            <input type="text" placeholder={placeholder} value={value}
                   onChange={(e) => onChange(e.target.value)}
                   className="flex-1 bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all font-mono"
            />
            {onClear && value && (
                <button type="button" onClick={onClear}
                        className="px-2.5 py-1.5 bg-neutral-800 border border-neutral-700/80 hover:bg-red-900/20 hover:border-red-900/50 hover:text-red-400 text-neutral-400 text-xs font-medium rounded-md transition-all whitespace-nowrap">
                    Clear
                </button>
            )}
            <button type="button" onClick={onBrowse}
                    className="px-2.5 py-1.5 bg-neutral-800 border border-neutral-700/80 hover:bg-neutral-700 hover:border-neutral-600 text-neutral-300 text-xs font-medium rounded-md transition-all whitespace-nowrap">
                {browseLabel}
            </button>
        </div>
    );
});

function SegmentedControl<T extends string>({ options, value, onChange }: {
    options: { value: T; label: string; description?: string }[];
    value: T; onChange: (v: T) => void;
}) {
    return (
        <div className="flex flex-col gap-0.5">
            {options.map((o) => (
                <button key={o.value} type="button" onClick={() => onChange(o.value)}
                        className={`flex items-center gap-2.5 px-2.5 py-1.5 rounded-md text-left transition-all ${
                            value === o.value
                                ? "bg-indigo-600/20 border border-indigo-500/40 text-indigo-300"
                                : "border border-transparent text-neutral-500 hover:bg-neutral-800/60 hover:text-neutral-300"
                        }`}>
                    <div className={`w-2.5 h-2.5 rounded-full border-2 shrink-0 transition-colors ${
                        value === o.value ? "border-indigo-400 bg-indigo-400" : "border-neutral-600"
                    }`} />
                    <div>
                        <span className="text-xs font-medium">{o.label}</span>
                        {o.description && <span className="text-[10px] text-neutral-600 ml-1.5">{o.description}</span>}
                    </div>
                </button>
            ))}
        </div>
    );
}

function PillTabs<T extends string>({ options, value, onChange }: {
    options: { value: T; label: string }[];
    value: T; onChange: (v: T) => void;
}) {
    return (
        <div className="flex flex-wrap gap-1">
            {options.map((o) => (
                <button key={o.value} type="button" onClick={() => onChange(o.value)}
                        className={`px-2.5 py-1 text-[11px] rounded-md font-medium transition-all ${
                            value === o.value
                                ? "bg-indigo-600 text-white shadow-sm shadow-indigo-900/50"
                                : "bg-neutral-800 text-neutral-500 hover:text-neutral-300 border border-neutral-700/60 hover:border-neutral-600"
                        }`}>
                    {o.label}
                </button>
            ))}
        </div>
    );
}

function Section({ title, defaultOpen = true, children }: {
    title: string; defaultOpen?: boolean; children: React.ReactNode;
}) {
    const [open, setOpen] = useState(defaultOpen);
    return (
        <div className="space-y-0">
            <button type="button" onClick={() => setOpen((v) => !v)} className="flex items-center justify-between w-full group">
                <h4 className="text-[10px] font-bold text-neutral-500 uppercase tracking-[0.15em]">{title}</h4>
                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"
                     className={`text-neutral-600 transition-transform duration-200 ${open ? "rotate-180" : ""}`}>
                    <path d="M2 3.5l3 3 3-3" />
                </svg>
            </button>
            <Divider />
            {open && <div className="pt-3 space-y-3">{children}</div>}
        </div>
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// useVideoMeta
// ─────────────────────────────────────────────────────────────────────────────

function useVideoMeta(path: string | undefined): { meta: VideoMeta | null; loading: boolean; error: string | null } {
    const [meta, setMeta] = useState<VideoMeta | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (!path) { setMeta(null); setError(null); return; }
        let cancelled = false;
        setLoading(true);
        setMeta(null);
        setError(null);

        const videoSrc = convertFileSrc(path);

        const tryHtmlVideo = () => new Promise<void>((resolve) => {
            const vid = document.createElement("video");
            vid.crossOrigin = "anonymous";
            vid.src = videoSrc;
            vid.muted = true;
            vid.preload = "metadata";
            vid.onloadedmetadata = () => { vid.currentTime = Math.min(1.5, vid.duration * 0.1); };
            vid.onseeked = () => {
                try {
                    const c = document.createElement("canvas");
                    c.width = vid.videoWidth; c.height = vid.videoHeight;
                    c.getContext("2d")?.drawImage(vid, 0, 0);
                    if (!cancelled) {
                        setMeta({
                            width: vid.videoWidth, height: vid.videoHeight, duration: vid.duration,
                            videoSrc,
                            posterDataUrl: c.toDataURL("image/jpeg", 0.75),
                        });
                    }
                } catch {
                    // Canvas tainted (cross-origin) — still set meta without a poster
                    if (!cancelled) {
                        setMeta({
                            width: vid.videoWidth, height: vid.videoHeight, duration: vid.duration,
                            videoSrc,
                            posterDataUrl: "",
                        });
                    }
                }
                vid.remove(); resolve();
            };
            vid.onerror = () => {
                vid.remove();
                if (!cancelled) setError("Could not load video preview.");
                resolve();
            };
        });

        const tryTauriCommand = async () => {
            try {
                const r = await invoke<{ width: number; height: number; duration: number; frameBase64: string }>(
                    "extract_video_frame", { path, seekSecs: 1.0 }
                );
                if (!cancelled) {
                    setMeta({
                        width: r.width, height: r.height, duration: r.duration,
                        videoSrc,
                        posterDataUrl: `data:image/jpeg;base64,${r.frameBase64}`,
                    });
                }
            } catch {
                await tryHtmlVideo();
            }
        };

        tryTauriCommand().finally(() => { if (!cancelled) setLoading(false); });
        return () => { cancelled = true; };
    }, [path]);

    return { meta, loading, error };
}

// ─────────────────────────────────────────────────────────────────────────────
// CanvasEditor — video player + draggable/resizable overlay editor
// ─────────────────────────────────────────────────────────────────────────────

interface CanvasEditorProps {
    bgWidth: number;
    bgHeight: number;
    videoMeta: VideoMeta | null;
    /** Only true when Direct Pipe Mode is enabled — controls whether the video preview renders */
    isOverlayMode: boolean;
    shapeOverlays: CustomShapeOverlay[];
    imageOverlays: CustomImageOverlay[];
    chatX: number;
    chatY: number;
    chatW: number;
    chatH: number;
    onChatChange: (x: number, y: number, w: number, h: number) => void;
    onShapeChange: (i: number, patch: Partial<CustomShapeOverlay>) => void;
    onImageChange: (i: number, patch: Partial<CustomImageOverlay>) => void;
    onShapeAdd: () => void;
    onShapeRemove: (i: number) => void;
    onImageAdd: () => void;
    onImageRemove: (i: number) => void;
    onImageBrowse: (i: number) => void;
}

// Checkerboard pattern is a constant — define once outside component to avoid re-creating
const CHECKERBOARD_STYLE: React.CSSProperties = {
    backgroundImage: "repeating-conic-gradient(#1a1a1a 0% 25%, #111 0% 50%)",
    backgroundSize: "20px 20px",
};

// Edge cursor map — stable constant, no per-render allocation
const EDGE_CURSOR: Record<string, string> = {
    n: "n-resize", s: "s-resize", e: "e-resize", w: "w-resize",
    ne: "ne-resize", nw: "nw-resize", se: "se-resize", sw: "sw-resize",
};

function CanvasEditor({
                          bgWidth, bgHeight,
                          videoMeta,
                          isOverlayMode,
                          shapeOverlays, imageOverlays,
                          chatX, chatY, chatW, chatH,
                          onChatChange, onShapeChange, onImageChange,
                          onShapeAdd, onShapeRemove, onImageAdd, onImageRemove, onImageBrowse,
                      }: CanvasEditorProps) {
    const containerRef = useRef<HTMLDivElement>(null);
    const stageRef = useRef<HTMLDivElement>(null);
    const videoRef = useRef<HTMLVideoElement>(null);
    const animRef = useRef<number>(0);

    const [editMode, setEditMode] = useState<EditMode>("chat");
    const [selected, setSelected] = useState<{ kind: OverlayKind; index?: number } | null>(null);
    const [isPlaying, setIsPlaying] = useState(false);
    const [currentTime, setCurrentTime] = useState(0);
    const [duration, setDuration] = useState(videoMeta?.duration ?? 0);
    const [videoReady, setVideoReady] = useState(false);
    const [isDragging, setIsDragging] = useState(false);

    const dragRef = useRef<DragState | null>(null);
    // Keep handlers and dimensions in a ref so the global pointer listener never closes over stale values
    const handlersRef = useRef({ onChatChange, onShapeChange, onImageChange, bgWidth, bgHeight });

    // Keep modes constrained — video scrub only available when Direct Pipe Mode is on with a video loaded
    useEffect(() => {
        if ((!videoMeta || !isOverlayMode) && editMode === "video") setEditMode("chat");
    }, [videoMeta, isOverlayMode, editMode]);

    // Sync latest callbacks/dimensions without re-subscribing the global listeners
    useEffect(() => {
        handlersRef.current = { onChatChange, onShapeChange, onImageChange, bgWidth, bgHeight };
    });

    // Window-level Drag Subscriptions
    useEffect(() => {
        const onMove = (e: PointerEvent) => {
            const d = dragRef.current;
            if (!d) return;

            if (d.kind === "scrub") {
                if (d.initTime === undefined || !d.initScreenW) return;
                const deltaX = e.clientX - d.startX;
                const deltaPct = deltaX / d.initScreenW;
                // Scale scrub rate (2 minutes full-width drag for precision)
                const scrubSensitivity = Math.min(duration, 120);
                const newTime = Math.max(0, Math.min(duration, d.initTime + deltaPct * scrubSensitivity));
                seek(newTime);
                return;
            }

            const { onChatChange: cC, onShapeChange: sC, onImageChange: iC, bgWidth: bW, bgHeight: bH } = handlersRef.current;
            const dx = d.initScreenW ? ((e.clientX - d.startX) / d.initScreenW) * bW : 0;
            const dy = d.initScreenH ? ((e.clientY - d.startY) / d.initScreenH) * bH : 0;

            let { x, y, width, height } = d.initRect;

            if (!d.edge) {
                // 1. Moving Constraints
                x = Math.round(x + dx);
                y = Math.round(y + dy);

                // Clamp X and Y so the box stays entirely within 0 and bW/bH
                x = Math.max(0, Math.min(x, bW - width));
                y = Math.max(0, Math.min(y, bH - height));
            } else {
                // 2. Resizing Constraints
                if (d.edge.includes("e")) {
                    width = Math.max(20, Math.round(d.initRect.width + dx));
                    // Constrain right edge to container width
                    width = Math.min(width, bW - d.initRect.x);
                }
                if (d.edge.includes("s")) {
                    height = Math.max(20, Math.round(d.initRect.height + dy));
                    // Constrain bottom edge to container height
                    height = Math.min(height, bH - d.initRect.y);
                }
                if (d.edge.includes("w")) {
                    let nw = Math.max(20, Math.round(d.initRect.width - dx));
                    // Constrain left edge to x >= 0
                    nw = Math.min(nw, d.initRect.x + d.initRect.width);
                    x = Math.round(d.initRect.x + d.initRect.width - nw);
                    width = nw;
                }
                if (d.edge.includes("n")) {
                    let nh = Math.max(20, Math.round(d.initRect.height - dy));
                    // Constrain top edge to y >= 0
                    nh = Math.min(nh, d.initRect.y + d.initRect.height);
                    y = Math.round(d.initRect.y + d.initRect.height - nh);
                    height = nh;
                }
            }

            if (d.kind === "chat") cC(x, y, width, height);
            else if (d.kind === "shape" && d.index !== undefined) sC(d.index, { x, y, width, height });
            else if (d.kind === "image" && d.index !== undefined) iC(d.index, { x, y, width: width || undefined, height: height || undefined });
        };

        const onUp = () => {
            if (dragRef.current) {
                dragRef.current = null;
                setIsDragging(false);
            }
        };

        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        window.addEventListener("pointercancel", onUp);

        return () => {
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            window.removeEventListener("pointercancel", onUp);
        };
    }, [duration]); // only re-subscribe when duration changes (needed for scrub calc)

    const onPointerDown = useCallback((
        e: React.PointerEvent,
        kind: OverlayKind,
        index: number | undefined,
        rect: { x: number; y: number; width: number; height: number },
        edge: DragState["edge"] = null,
    ) => {
        e.preventDefault();
        e.stopPropagation();
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);

        let sw = 1; let sh = 1;
        if (stageRef.current) {
            const r = stageRef.current.getBoundingClientRect();
            sw = Math.max(1, r.width); sh = Math.max(1, r.height);
        }

        dragRef.current = {
            kind, index, edge,
            startX: e.clientX, startY: e.clientY,
            initRect: rect,
            initBgW: handlersRef.current.bgWidth,
            initBgH: handlersRef.current.bgHeight,
            initScreenW: sw, initScreenH: sh,
        };
        setSelected({ kind, index });
        setIsDragging(true);
    }, []);

    const handleModeChange = useCallback((m: EditMode) => {
        setEditMode(m);
        setSelected(m === "chat" ? { kind: "chat" } : null);
    }, []);

    useEffect(() => {
        setIsPlaying(false);
        setCurrentTime(0);
        setVideoReady(false);
        setDuration(videoMeta?.duration ?? 0);
    }, [videoMeta?.videoSrc]);

    useEffect(() => {
        if (!isPlaying) { cancelAnimationFrame(animRef.current); return; }
        const tick = () => {
            if (videoRef.current) setCurrentTime(videoRef.current.currentTime);
            animRef.current = requestAnimationFrame(tick);
        };
        animRef.current = requestAnimationFrame(tick);
        return () => cancelAnimationFrame(animRef.current);
    }, [isPlaying]);

    const togglePlay = useCallback(() => {
        if (!videoRef.current || !videoReady) return;
        if (isPlaying) { videoRef.current.pause(); setIsPlaying(false); }
        else { videoRef.current.play().then(() => setIsPlaying(true)).catch(() => {}); }
    }, [isPlaying, videoReady]);

    const seek = (t: number) => {
        if (!videoRef.current) return;
        videoRef.current.currentTime = t;
        setCurrentTime(t);
    };

    const handleSeekChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
        seek(Number(e.target.value));
    }, []);

    const handleVideoMetadata = useCallback((e: React.SyntheticEvent<HTMLVideoElement>) => {
        setDuration((e.currentTarget as HTMLVideoElement).duration);
        setVideoReady(true);
    }, []);

    const handleVideoEnded = useCallback(() => setIsPlaying(false), []);

    const pct = (v: number, dim: number) => `${(v / dim) * 100}%`;

    // OverlayHandle is defined at module level (above CanvasEditor) so React
    // always sees the same component type — no unmount/remount on re-render.

    const selKind = selected?.kind;
    const selIdx = selected?.index;

    const selShape = selKind === "shape" && selIdx !== undefined ? shapeOverlays[selIdx] : null;
    const selImage = selKind === "image" && selIdx !== undefined ? imageOverlays[selIdx] : null;

    // Stable container-level pointer-down handler
    const handleContainerPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
        const target = e.target as HTMLElement;
        const isBackground = target === stageRef.current || target.classList.contains("bg-checkerboard");
        if (editMode === "video" && videoMeta && isBackground) {
            e.preventDefault();
            let sw = 1; let sh = 1;
            if (stageRef.current) {
                const r = stageRef.current.getBoundingClientRect();
                sw = Math.max(1, r.width); sh = Math.max(1, r.height);
            }
            dragRef.current = {
                kind: "scrub", startX: e.clientX, startY: e.clientY, edge: null,
                initRect: { x: 0, y: 0, width: 0, height: 0 }, initTime: currentTime,
                initBgW: bgWidth, initBgH: bgHeight, initScreenW: sw, initScreenH: sh,
            };
            setIsDragging(true);
        } else if (isBackground) {
            setSelected(null);
        }
    }, [editMode, videoMeta, currentTime, bgWidth, bgHeight]);

    const containerStyle: React.CSSProperties = useMemo(() => ({
        // Keep the editor in a normal streamer/OBS-style 16:9 viewport.
        // The logical render canvas is fitted inside this viewport so its native
        // coordinates stay intact even when a reference video is portrait/tall.
        aspectRatio: "16 / 9",
        cursor: isDragging ? "grabbing" : (editMode === "video" ? "ew-resize" : "default"),
        touchAction: "none",
    }), [isDragging, editMode]);

    const stageStyle = useMemo<React.CSSProperties>(() => {
        const outerRatio = 16 / 9;
        const logicalRatio = bgWidth / Math.max(1, bgHeight);
        if (logicalRatio >= outerRatio) {
            return {
                position: "absolute",
                left: 0,
                right: 0,
                top: "50%",
                height: `${(outerRatio / logicalRatio) * 100}%`,
                transform: "translateY(-50%)",
            };
        }
        return {
            position: "absolute",
            top: 0,
            bottom: 0,
            left: "50%",
            width: `${(logicalRatio / outerRatio) * 100}%`,
            transform: "translateX(-50%)",
        };
    }, [bgWidth, bgHeight]);

    return (
        <div className="space-y-3">
            {/* ── Canvas viewport ─────────────────────────────────────────────── */}
            <div
                ref={containerRef}
                className="relative w-full rounded-lg overflow-hidden border border-neutral-700 bg-neutral-950 select-none"
                style={containerStyle}
                onPointerDown={handleContainerPointerDown}
            >
                <div
                    ref={stageRef}
                    className="absolute overflow-visible"
                    style={stageStyle}
                >
                    {/* Top Canvas Toolbar */}
                    <div className="absolute top-2 left-1/2 -translate-x-1/2 flex items-center bg-neutral-950/90 backdrop-blur-sm border border-neutral-700/80 p-1 rounded-lg z-50 shadow-xl gap-0.5" style={{ whiteSpace: "nowrap" }}>
                        {isOverlayMode && videoMeta && (
                            <button type="button" onClick={(e) => { e.stopPropagation(); handleModeChange("video"); }}
                                    title="Drag canvas left/right to scrub video"
                                    className={`flex items-center gap-1 px-2.5 py-1.5 text-[9px] uppercase tracking-wider font-bold rounded-md transition-colors ${editMode === "video" ? "bg-indigo-600 text-white shadow-sm" : "text-neutral-500 hover:text-neutral-200 hover:bg-neutral-800"}`}>
                                <svg width="8" height="8" viewBox="0 0 10 10" fill="currentColor"><path d="M1 5h8M6 2l3 3-3 3M4 2 1 5l3 3"/></svg>
                                Scrub
                            </button>
                        )}
                        <button type="button" onClick={(e) => { e.stopPropagation(); handleModeChange("chat"); }}
                                title="Move and resize the chat overlay region"
                                className={`flex items-center gap-1 px-2.5 py-1.5 text-[9px] uppercase tracking-wider font-bold rounded-md transition-colors ${editMode === "chat" ? "bg-violet-600 text-white shadow-sm" : "text-neutral-500 hover:text-neutral-200 hover:bg-neutral-800"}`}>
                            <svg width="8" height="8" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5"><rect x="1" y="1" width="8" height="8" rx="1"/></svg>
                            Chat
                        </button>
                        <button type="button" onClick={(e) => { e.stopPropagation(); handleModeChange("shapes"); }}
                                title="Select, move and resize shape and image overlays"
                                className={`flex items-center gap-1 px-2.5 py-1.5 text-[9px] uppercase tracking-wider font-bold rounded-md transition-colors ${editMode === "shapes" ? "bg-emerald-600 text-white shadow-sm" : "text-neutral-500 hover:text-neutral-200 hover:bg-neutral-800"}`}>
                            <svg width="8" height="8" viewBox="0 0 10 10" fill="currentColor"><circle cx="5" cy="5" r="3"/></svg>
                            Shapes
                        </button>
                    </div>

                    {/* Background — only show video preview when Direct Pipe Mode is ON */}
                    {isOverlayMode && videoMeta ? (
                        <video
                            ref={videoRef}
                            src={videoMeta.videoSrc}
                            poster={videoMeta.posterDataUrl}
                            className="absolute inset-0 w-full h-full object-contain pointer-events-none"
                            style={{ opacity: 0.6 }}
                            muted
                            playsInline
                            onLoadedMetadata={handleVideoMetadata}
                            onEnded={handleVideoEnded}
                        />
                    ) : (
                        <div className="absolute inset-0 pointer-events-none bg-checkerboard" style={CHECKERBOARD_STYLE} />
                    )}

                    {/* Shape overlays */}
                    {shapeOverlays.map((s, i) => {
                        const isSel = selKind === "shape" && selIdx === i;
                        const rect = { x: s.x, y: s.y, width: s.width, height: s.height };
                        return (
                            <div key={`shape-${i}`}
                                 style={{
                                     position: "absolute",
                                     left: pct(s.x, bgWidth), top: pct(s.y, bgHeight),
                                     width: pct(s.width, bgWidth), height: pct(s.height, bgHeight),
                                     backgroundColor: `rgba(${s.color.red},${s.color.green},${s.color.blue},${s.color.alpha / 255})`,
                                     borderRadius: s.cornerRadius > 0 ? `${(s.cornerRadius / Math.min(s.width, s.height)) * 50}%` : 0,
                                     border: isSel ? "1.5px solid rgba(99,102,241,0.9)" : "1.5px dashed rgba(99,102,241,0.5)",
                                     cursor: "move", zIndex: 10, boxSizing: "border-box", userSelect: "none",
                                     pointerEvents: editMode === "shapes" ? "auto" : "none",
                                     touchAction: "none",
                                 }}
                                 onPointerDown={(e) => { onPointerDown(e, "shape", i, rect); }}
                            >
                            <span style={{ position: "absolute", top: 2, left: 4, fontSize: 8, fontWeight: 700, color: "rgba(165,180,252,0.7)", pointerEvents: "none", userSelect: "none", lineHeight: 1 }}>
                                S{i + 1}
                            </span>
                                {isSel && EDGES.map((edge) => <OverlayHandle key={edge} edge={edge} rect={rect} kind="shape" index={i} onPointerDown={onPointerDown} />)}
                            </div>
                        );
                    })}

                    {/* Image overlays */}
                    {imageOverlays.map((ov, i) => {
                        const isSel = selKind === "image" && selIdx === i;
                        const w = ov.width ?? 200; const h = ov.height ?? 150;
                        const rect = { x: ov.x, y: ov.y, width: w, height: h };
                        // convertFileSrc in render is fine here — it's synchronous and cheap
                        const bgImage = ov.assetPath ? `url(${convertFileSrc(ov.assetPath)})` : undefined;
                        return (
                            <div key={`image-${i}`}
                                 style={{
                                     position: "absolute",
                                     left: pct(ov.x, bgWidth), top: pct(ov.y, bgHeight),
                                     width: pct(w, bgWidth), height: pct(h, bgHeight),
                                     opacity: ov.alpha,
                                     backgroundImage: bgImage,
                                     backgroundSize: "cover", backgroundPosition: "center",
                                     backgroundColor: ov.assetPath ? undefined : "rgba(52,211,153,0.1)",
                                     border: isSel ? "1.5px solid rgba(52,211,153,0.9)" : "1.5px dashed rgba(52,211,153,0.5)",
                                     cursor: "move", zIndex: 11, boxSizing: "border-box", userSelect: "none",
                                     pointerEvents: editMode === "shapes" ? "auto" : "none",
                                     touchAction: "none",
                                 }}
                                 onPointerDown={(e) => { onPointerDown(e, "image", i, rect); }}
                            >
                                {!ov.assetPath && (
                                    <span style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 9, fontWeight: 700, color: "rgba(52,211,153,0.6)", pointerEvents: "none", userSelect: "none" }}>
                                    IMG {i + 1}
                                </span>
                                )}
                                {isSel && EDGES.map((edge) => <OverlayHandle key={edge} edge={edge} rect={rect} kind="image" index={i} onPointerDown={onPointerDown} />)}
                            </div>
                        );
                    })}

                    {/* Chat overlay box */}
                    {(() => {
                        const isSel = selKind === "chat";
                        const isActiveMode = editMode === "chat";
                        const rect = { x: chatX, y: chatY, width: chatW, height: chatH };
                        return (
                            <div
                                style={{
                                    position: "absolute",
                                    left: pct(chatX, bgWidth), top: pct(chatY, bgHeight),
                                    width: pct(chatW, bgWidth), height: pct(chatH, bgHeight),
                                    backgroundColor: isSel ? "rgba(109,40,217,0.22)" : "rgba(109,40,217,0.12)",
                                    border: isSel
                                        ? "2px solid rgba(139,92,246,0.95)"
                                        : isActiveMode ? "2px dashed rgba(139,92,246,0.6)" : "2px dashed rgba(139,92,246,0.2)",
                                    cursor: isActiveMode ? "move" : "default",
                                    zIndex: 20,
                                    display: "flex", alignItems: "center", justifyContent: "center",
                                    boxSizing: "border-box", userSelect: "none",
                                    pointerEvents: isActiveMode ? "auto" : "none",
                                    touchAction: "none",
                                    transition: "border-color 0.15s, background-color 0.15s",
                                }}
                                onPointerDown={(e) => { onPointerDown(e, "chat", undefined, rect); }}
                            >
                                <div style={{ pointerEvents: "none", userSelect: "none", textAlign: "center" }}>
                                    <div style={{
                                        fontSize: 9, fontWeight: 700,
                                        color: isActiveMode ? "rgba(221,214,254,0.95)" : "rgba(167,139,250,0.4)",
                                        background: isActiveMode ? "rgba(109,40,217,0.75)" : "rgba(109,40,217,0.3)",
                                        padding: "2px 6px", borderRadius: 3, lineHeight: 1.5,
                                        display: "flex", alignItems: "center", gap: 4,
                                    }}>
                                        {isActiveMode && (
                                            <svg width="7" height="7" viewBox="0 0 10 10" fill="currentColor" style={{ opacity: 0.8 }}>
                                                <path d="M1 1h3v3H1zM6 1h3v3H6zM1 6h3v3H1zM6 6h3v3H6z"/>
                                            </svg>
                                        )}
                                        CHAT OVERLAY
                                    </div>
                                    {isSel && <div style={{ fontSize: 8, color: "rgba(196,181,253,0.6)", fontFamily: "monospace", marginTop: 2 }}>{chatW}×{chatH} · drag to move</div>}
                                </div>
                                {isSel && EDGES.map((edge) => <OverlayHandle key={edge} edge={edge} rect={rect} kind="chat" onPointerDown={onPointerDown} />)}
                            </div>
                        );
                    })()}

                    {/* Mode hint overlays */}
                    {editMode === "video" && (
                        <div style={{ position: "absolute", bottom: 6, left: "50%", transform: "translateX(-50%)", fontSize: 9, color: "rgba(255,255,255,0.9)", background: "rgba(10,10,10,0.7)", padding: "2px 8px", borderRadius: 4, pointerEvents: "none", userSelect: "none", whiteSpace: "nowrap" }}>
                            Drag horizontally to scrub video
                        </div>
                    )}
                    {editMode === "chat" && (
                        <div style={{ position: "absolute", bottom: 6, left: "50%", transform: "translateX(-50%)", fontSize: 9, color: "rgba(196,181,253,0.9)", background: "rgba(10,10,10,0.7)", padding: "2px 8px", borderRadius: 4, pointerEvents: "none", userSelect: "none", whiteSpace: "nowrap" }}>
                            Drag chat box · grab handles to resize
                        </div>
                    )}
                    {editMode === "shapes" && (
                        <div style={{ position: "absolute", bottom: 6, left: "50%", transform: "translateX(-50%)", fontSize: 9, color: "rgba(110,231,183,0.9)", background: "rgba(10,10,10,0.7)", padding: "2px 8px", borderRadius: 4, pointerEvents: "none", userSelect: "none", whiteSpace: "nowrap" }}>
                            Click a shape or image to select · drag to move · grab handles to resize
                        </div>
                    )}
                    {!isOverlayMode && (
                        <div style={{ position: "absolute", top: 36, right: 6, fontSize: 8, color: "rgba(115,115,115,0.7)", background: "rgba(10,10,10,0.6)", padding: "1px 6px", borderRadius: 3, pointerEvents: "none", userSelect: "none" }}>
                            Preview: standalone canvas
                        </div>
                    )}

                    <div style={{ position: "absolute", top: 6, left: 6, fontSize: 9, fontFamily: "monospace", color: "rgba(115,115,115,0.9)", background: "rgba(10,10,10,0.7)", padding: "1px 5px", borderRadius: 4, pointerEvents: "none", userSelect: "none" }}>
                        {bgWidth}×{bgHeight}
                        {videoMeta && videoMeta.width !== bgWidth && ` · vid ${videoMeta.width}×${videoMeta.height}`}
                    </div>
                </div>
            </div>

            {/* ── Video transport controls ────────────────────────────────────── */}
            {isOverlayMode && videoMeta && (
                <div className="flex items-center gap-2.5 px-1">
                    <button type="button" onClick={togglePlay} disabled={!videoReady}
                            className="w-7 h-7 shrink-0 flex items-center justify-center rounded-md bg-neutral-800 hover:bg-neutral-700 text-neutral-300 disabled:opacity-40 disabled:cursor-not-allowed transition-colors">
                        {isPlaying
                            ? <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor"><rect x="1.5" y="1" width="3" height="8" /><rect x="5.5" y="1" width="3" height="8" /></svg>
                            : <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor"><path d="M2 1l7 4-7 4V1z" /></svg>
                        }
                    </button>
                    <input type="range" min={0} max={duration || 1} step={0.033} value={currentTime}
                           onChange={handleSeekChange}
                           disabled={!videoReady}
                           className="flex-1 h-1 accent-violet-500 disabled:opacity-40"
                    />
                    <span className="text-[10px] font-mono text-neutral-600 shrink-0 w-16 text-right">
                        {formatTime(currentTime)} / {formatTime(duration)}
                    </span>
                </div>
            )}

            {/* ── Add overlay buttons ──────────────────────────────────────────── */}
            <div className="flex gap-2">
                <button type="button" onClick={() => { setEditMode("shapes"); onShapeAdd(); }}
                        className="flex-1 py-1.5 border border-dashed border-indigo-800/60 hover:border-indigo-500/70 rounded-lg text-[10px] text-indigo-500/70 hover:text-indigo-400 transition-all flex items-center justify-center gap-1">
                    <svg width="9" height="9" viewBox="0 0 9 9" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M4.5 1v7M1 4.5h7" /></svg>
                    Add shape
                </button>
                <button type="button" onClick={() => { setEditMode("shapes"); onImageAdd(); }}
                        className="flex-1 py-1.5 border border-dashed border-emerald-800/60 hover:border-emerald-500/70 rounded-lg text-[10px] text-emerald-500/70 hover:text-emerald-400 transition-all flex items-center justify-center gap-1">
                    <svg width="9" height="9" viewBox="0 0 9 9" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M4.5 1v7M1 4.5h7" /></svg>
                    Add image
                </button>
            </div>

            {/* ── Inspector panel — selected overlay ───────────────────────────── */}
            {selected && (
                <div className="bg-neutral-900/80 border border-neutral-700/60 rounded-lg px-3 py-2.5 space-y-2.5">
                    {/* Chat */}
                    {selKind === "chat" && (
                        <>
                            <div className="flex items-center justify-between mb-0.5">
                                <span className="text-[10px] font-bold text-violet-400 uppercase tracking-wide">Chat overlay</span>
                            </div>
                            <div className="grid grid-cols-4 gap-1.5">
                                <div><FieldLabel>X</FieldLabel><NumberInput value={chatX} onChange={(v) => onChatChange(v, chatY, chatW, chatH)} /></div>
                                <div><FieldLabel>Y</FieldLabel><NumberInput value={chatY} onChange={(v) => onChatChange(chatX, v, chatW, chatH)} /></div>
                                <div><FieldLabel>W</FieldLabel><NumberInput value={chatW} min={1} onChange={(v) => onChatChange(chatX, chatY, v, chatH)} /></div>
                                <div><FieldLabel>H</FieldLabel><NumberInput value={chatH} min={1} onChange={(v) => onChatChange(chatX, chatY, chatW, v)} /></div>
                            </div>
                        </>
                    )}

                    {/* Shape */}
                    {selKind === "shape" && selShape && selIdx !== undefined && (
                        <>
                            <div className="flex items-center justify-between mb-0.5">
                                <span className="text-[10px] font-bold text-indigo-400 uppercase tracking-wide">Shape {selIdx + 1}</span>
                                <button type="button" onClick={() => { onShapeRemove(selIdx); setSelected(null); }}
                                        className="text-[10px] text-neutral-600 hover:text-red-400 transition-colors">Remove</button>
                            </div>
                            <div className="grid grid-cols-4 gap-1.5">
                                <div><FieldLabel>X</FieldLabel><NumberInput value={selShape.x} onChange={(v) => onShapeChange(selIdx, { x: v })} /></div>
                                <div><FieldLabel>Y</FieldLabel><NumberInput value={selShape.y} onChange={(v) => onShapeChange(selIdx, { y: v })} /></div>
                                <div><FieldLabel>W</FieldLabel><NumberInput value={selShape.width} min={1} onChange={(v) => onShapeChange(selIdx, { width: v })} /></div>
                                <div><FieldLabel>H</FieldLabel><NumberInput value={selShape.height} min={1} onChange={(v) => onShapeChange(selIdx, { height: v })} /></div>
                            </div>
                            <div className="grid grid-cols-2 gap-2 pt-1">
                                <div><FieldLabel>Corner radius</FieldLabel><NumberInput value={selShape.cornerRadius} min={0} onChange={(v) => onShapeChange(selIdx, { cornerRadius: v })} /></div>
                                <div><FieldLabel>Opacity (0–255)</FieldLabel><NumberInput value={selShape.color.alpha} min={0} max={255} onChange={(v) => onShapeChange(selIdx, { color: { ...selShape.color, alpha: v } })} /></div>
                            </div>
                            <ColorPicker label="Color" value={selShape.color} onChange={(c) => onShapeChange(selIdx, { color: c })} />
                        </>
                    )}

                    {/* Image */}
                    {selKind === "image" && selImage && selIdx !== undefined && (
                        <>
                            <div className="flex items-center justify-between mb-0.5">
                                <span className="text-[10px] font-bold text-emerald-400 uppercase tracking-wide">Image {selIdx + 1}</span>
                                <button type="button" onClick={() => { onImageRemove(selIdx); setSelected(null); }}
                                        className="text-[10px] text-neutral-600 hover:text-red-400 transition-colors">Remove</button>
                            </div>
                            <div className="grid grid-cols-4 gap-1.5">
                                <div><FieldLabel>X</FieldLabel><NumberInput value={selImage.x} onChange={(v) => onImageChange(selIdx, { x: v })} /></div>
                                <div><FieldLabel>Y</FieldLabel><NumberInput value={selImage.y} onChange={(v) => onImageChange(selIdx, { y: v })} /></div>
                                <div><FieldLabel>W</FieldLabel><NumberInput value={selImage.width ?? 200} min={1} onChange={(v) => onImageChange(selIdx, { width: v })} /></div>
                                <div><FieldLabel>H</FieldLabel><NumberInput value={selImage.height ?? 150} min={1} onChange={(v) => onImageChange(selIdx, { height: v })} /></div>
                            </div>
                            <div>
                                <FieldLabel>Asset path</FieldLabel>
                                <BrowseInput value={selImage.assetPath} onChange={(v) => onImageChange(selIdx, { assetPath: v })}
                                             onBrowse={() => onImageBrowse(selIdx)} placeholder="/path/to/image.png" browseLabel="Browse" />
                            </div>
                            <CustomSlider label="Opacity" value={selImage.alpha} min={0} max={1} step={0.01} unit="" onChange={(v) => onImageChange(selIdx, { alpha: v })} />
                        </>
                    )}
                </div>
            )}

            {/* ── Overlay list ─────────────────────────────────────────────────── */}
            {(shapeOverlays.length > 0 || imageOverlays.length > 0) && (
                <div className="space-y-1 pt-0.5">
                    <p className="text-[10px] text-neutral-700 font-medium uppercase tracking-wide">Overlays — click to select &amp; edit</p>
                    {shapeOverlays.map((s, i) => (
                        <button key={`sl-${i}`} type="button"
                                onClick={() => { setEditMode("shapes"); setSelected({ kind: "shape", index: i }); }}
                                className={`w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-left transition-all ${
                                    selKind === "shape" && selIdx === i
                                        ? "bg-indigo-900/30 border border-indigo-700/50"
                                        : "bg-neutral-900/50 border border-neutral-800 hover:border-neutral-700"
                                }`}>
                            <div style={{ width: 12, height: 12, borderRadius: s.cornerRadius > 0 ? 3 : 1, background: `rgba(${s.color.red},${s.color.green},${s.color.blue},${s.color.alpha / 255})`, border: "1px solid rgba(99,102,241,0.4)", flexShrink: 0 }} />
                            <span className="text-[11px] text-neutral-400 font-medium">Shape {i + 1}</span>
                            <span className="text-[10px] text-neutral-700 font-mono ml-auto">{s.x},{s.y} · {s.width}×{s.height}</span>
                        </button>
                    ))}
                    {imageOverlays.map((ov, i) => (
                        <button key={`il-${i}`} type="button"
                                onClick={() => { setEditMode("shapes"); setSelected({ kind: "image", index: i }); }}
                                className={`w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-left transition-all ${
                                    selKind === "image" && selIdx === i
                                        ? "bg-emerald-900/20 border border-emerald-700/40"
                                        : "bg-neutral-900/50 border border-neutral-800 hover:border-neutral-700"
                                }`}>
                            <div style={{ width: 12, height: 12, borderRadius: 2, border: "1px dashed rgba(52,211,153,0.5)", background: ov.assetPath ? `url(${convertFileSrc(ov.assetPath)}) center/cover` : "rgba(52,211,153,0.1)", flexShrink: 0 }} />
                            <span className="text-[11px] text-neutral-400 font-medium">Image {i + 1}</span>
                            {ov.assetPath && <span className="text-[10px] text-neutral-700 font-mono truncate max-w-30">{ov.assetPath.split(/[/\\]/).pop()}</span>}
                            <span className="text-[10px] text-neutral-700 font-mono ml-auto">{ov.x},{ov.y} · {ov.width ?? "auto"}×{ov.height ?? "auto"}</span>
                        </button>
                    ))}
                </div>
            )}
        </div>
    );
}

function formatTime(s: number) {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60);
    return `${m}:${String(sec).padStart(2, "0")}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// ResetChip — module-level so React sees a stable component type.
// Defined here (not inside SettingsPanel) to avoid unmount/remount on every
// SettingsPanel render when overriddenKeys or onResetKey change.
// ─────────────────────────────────────────────────────────────────────────────

const ResetChip = memo(function ResetChip({ fieldKey, overriddenKeys, onResetKey }: {
    fieldKey: string;
    overriddenKeys?: Set<string>;
    onResetKey?: (key: string) => void;
}) {
    if (!overriddenKeys?.has(fieldKey) || !onResetKey) return null;
    return (
        <button type="button" onClick={() => onResetKey(fieldKey)}
                className="ml-1.5 text-[9px] px-1.5 py-0.5 rounded bg-amber-900/40 text-amber-400 border border-amber-700/40 hover:bg-amber-900/70 transition-colors leading-none">
            override · reset
        </button>
    );
});

// ─────────────────────────────────────────────────────────────────────────────
// Shared settings panel
// ─────────────────────────────────────────────────────────────────────────────

interface SettingsPanelProps {
    opts: RenderVideoArgs;
    setOpt: <K extends keyof RenderVideoArgs>(key: K, value: RenderVideoArgs[K]) => void;
    setOpts: (patch: Partial<RenderVideoArgs>) => void;
    overriddenKeys?: Set<string>;
    onResetKey?: (key: string) => void;
    isBatchItem?: boolean;
    overlayVideoPath?: string;
    onSelectOverlayVideo?: () => void;
    videoMeta?: VideoMeta | null;
    videoLoading?: boolean;
    videoError?: string | null;
    onSetOverlayVideoPath?: (v: string) => void;
    /** Batch shared-template mode: show a canvas even without a shared base video. */
    showTemplateCanvas?: boolean;
    /** In batch template mode this video is reference-only and is never sent to the renderer. */
    previewOnlyBaseVideo?: boolean;
}

function SettingsPanel({
                           opts, setOpt, setOpts,
                           overriddenKeys, onResetKey,
                           isBatchItem = false,
                           overlayVideoPath, onSelectOverlayVideo, videoMeta, videoLoading, videoError, onSetOverlayVideoPath,
                           showTemplateCanvas = false, previewOnlyBaseVideo = false,
                       }: SettingsPanelProps) {
    const [pinnedRaw, setPinnedRaw] = useState(() => opts.pinnedUsers.join(", "));
    const [skipRaw, setSkipRaw] = useState(() => opts.skipUsers.join(", "));

    // Sync raw strings when the underlying array changes (e.g. batch reset / copy).
    // Compare the joined value rather than array identity so that semantically
    // identical arrays (e.g. produced by copySettingsFromItem) don't skip the sync.
    useEffect(() => {
        const joined = opts.pinnedUsers.join(", ");
        setPinnedRaw((prev) => (prev !== joined ? joined : prev));
    }, [opts.pinnedUsers]);
    useEffect(() => {
        const joined = opts.skipUsers.join(", ");
        setSkipRaw((prev) => (prev !== joined ? joined : prev));
    }, [opts.skipUsers]);

    const onChatChange = useCallback((x: number, y: number, w: number, h: number) => {
        // Chat geometry is an overlay property; resizing it must never resize the actual render canvas.
        setOpts({ overlayX: x, overlayY: y, overlayWidth: w, overlayHeight: h });
    }, [setOpts]);

    const flush = useCallback((key: "pinnedUsers" | "skipUsers", raw: string) => {
        setOpt(key, raw.split(",").map((s) => s.trim()).filter(Boolean));
    }, [setOpt]);

    // ResetChip is defined at module level — pass overriddenKeys and onResetKey as props.

    const setShapes = useCallback((v: CustomShapeOverlay[]) => setOpt("shapeOverlays", v), [setOpt]);
    const setImages = useCallback((v: CustomImageOverlay[]) => setOpt("imageOverlays", v), [setOpt]);

    const handleShapeChange = useCallback((i: number, patch: Partial<CustomShapeOverlay>) => {
        const next = [...opts.shapeOverlays]; next[i] = { ...next[i], ...patch }; setShapes(next);
    }, [opts.shapeOverlays, setShapes]);

    const handleImageChange = useCallback((i: number, patch: Partial<CustomImageOverlay>) => {
        const next = [...opts.imageOverlays]; next[i] = { ...next[i], ...patch }; setImages(next);
    }, [opts.imageOverlays, setImages]);

    const handleImageBrowse = useCallback(async (i: number) => {
        try {
            const sel = await open({ multiple: false, directory: false, filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }] });
            if (typeof sel === "string") handleImageChange(i, { assetPath: sel });
        } catch {}
    }, [handleImageChange]);

    const handleShapeAdd = useCallback(() =>
            setShapes([...opts.shapeOverlays, { x: 20, y: 20, width: Math.round(opts.width / 3), height: Math.round(opts.height / 6), color: { alpha: 200, red: 30, green: 30, blue: 30 }, cornerRadius: 8 }]),
        [opts.shapeOverlays, opts.width, opts.height, setShapes]);

    const handleShapeRemove = useCallback((i: number) =>
            setShapes(opts.shapeOverlays.filter((_, idx) => idx !== i)),
        [opts.shapeOverlays, setShapes]);

    const handleImageAdd = useCallback(() =>
            setImages([...opts.imageOverlays, { assetPath: "", x: 0, y: 0, width: Math.round(opts.width / 3), height: Math.round(opts.height / 5), alpha: 1.0 }]),
        [opts.imageOverlays, opts.width, opts.height, setImages]);

    const handleImageRemove = useCallback((i: number) =>
            setImages(opts.imageOverlays.filter((_, idx) => idx !== i)),
        [opts.imageOverlays, setImages]);

    const handleClearOverlayVideo = useCallback(() => onSetOverlayVideoPath?.(""), [onSetOverlayVideoPath]);

    const chatX = opts.overlayX ?? 0;
    const chatY = opts.overlayY ?? 0;
    const chatW = opts.overlayWidth ?? opts.width;
    const chatH = opts.overlayHeight ?? opts.height;

    const isOverlayMode = opts.useImmediatePipeOverlay;
    const previewVideoMeta = videoMeta;
    const bgWidth = (isOverlayMode && previewVideoMeta) ? previewVideoMeta.width : opts.width;
    const bgHeight = (isOverlayMode && previewVideoMeta) ? previewVideoMeta.height : opts.height;

    return (
        <>
            {/* ── Canvas + Overlays ─────────────────────────────────────────────── */}
            <Section title="Canvas & Overlays" defaultOpen>
                <div className="grid grid-cols-3 gap-2">
                    <div>
                        <div className="flex items-center mb-1"><FieldLabel>Width</FieldLabel><ResetChip fieldKey="width" overriddenKeys={overriddenKeys} onResetKey={onResetKey} /></div>
                        <NumberInput value={opts.width} min={1} onChange={(v) => setOpt("width", v)} />
                    </div>
                    <div>
                        <div className="flex items-center mb-1"><FieldLabel>Height</FieldLabel><ResetChip fieldKey="height" overriddenKeys={overriddenKeys} onResetKey={onResetKey} /></div>
                        <NumberInput value={opts.height} min={1} onChange={(v) => setOpt("height", v)} />
                    </div>
                    <div>
                        <div className="flex items-center mb-1"><FieldLabel>FPS</FieldLabel><ResetChip fieldKey="fps" overriddenKeys={overriddenKeys} onResetKey={onResetKey} /></div>
                        <NumberInput value={opts.fps} min={1} max={120} onChange={(v) => setOpt("fps", v)} />
                    </div>
                </div>

                <div className="mt-3 pt-2 border-t border-neutral-800/50 space-y-3">
                    <Toggle value={opts.useImmediatePipeOverlay} onChange={(v) => setOpt("useImmediatePipeOverlay", v)}
                            label="Direct Pipe Mode (Video Overlay)" description="Overlay chat directly onto a base video, bypassing temp files." />

                    {isOverlayMode && onSelectOverlayVideo && onSetOverlayVideoPath && (
                        <div>
                            <FieldLabel>{previewOnlyBaseVideo ? "Reference base video (preview only)" : "Base video for overlay"}</FieldLabel>
                            <BrowseInput value={overlayVideoPath ?? ""} onChange={onSetOverlayVideoPath}
                                         onBrowse={onSelectOverlayVideo} onClear={handleClearOverlayVideo}
                                         placeholder={previewOnlyBaseVideo ? "/optional/reference/stream.mp4" : "/path/to/stream.mp4"} browseLabel="Browse" />
                            {previewOnlyBaseVideo && (
                                <p className="text-[9px] text-neutral-700 mt-1.5">Reference only: item videos are still used for the actual batch renders.</p>
                            )}
                        </div>
                    )}

                    {isOverlayMode && overlayVideoPath && !videoLoading && (
                        <div>
                            <FieldLabel>Timeline mismatch strategy</FieldLabel>
                            <SegmentedControl options={TIMELINE_STRATEGIES} value={opts.timelineMismatchStrategy} onChange={(v) => setOpt("timelineMismatchStrategy", v)} />
                        </div>
                    )}
                </div>

                {videoLoading && (
                    <div className="mt-2 w-full h-32 rounded-lg bg-neutral-900/60 border border-neutral-700/50 flex flex-col items-center justify-center gap-2 text-neutral-500">
                        <span className="w-5 h-5 border-2 border-neutral-700 border-t-indigo-500 rounded-full animate-spin" />
                        <span className="text-[11px]">Loading video…</span>
                    </div>
                )}

                {!videoLoading && videoError && overlayVideoPath && (
                    <div className="mt-2 flex items-center gap-2 px-3 py-2 bg-red-950/30 border border-red-800/40 rounded-lg text-[11px] text-red-400">
                        <svg className="shrink-0" width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                            <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" />
                        </svg>
                        {videoError}
                    </div>
                )}

                {!videoLoading && isOverlayMode && (showTemplateCanvas || !!overlayVideoPath) && (
                    <div className="mt-3">
                        {showTemplateCanvas && !overlayVideoPath && (
                            <div className="mb-2 flex items-center gap-2 px-3 py-2 rounded-lg bg-violet-950/25 border border-violet-800/40 text-[10px] text-violet-300/80">
                                <span className="w-1.5 h-1.5 rounded-full bg-violet-400 shrink-0" />
                                Template preview · reference video is optional and never rendered; each item supplies its own base video.
                            </div>
                        )}
                        <CanvasEditor
                            bgWidth={bgWidth}
                            bgHeight={bgHeight}
                            videoMeta={previewVideoMeta ?? null}
                            isOverlayMode={isOverlayMode}
                            shapeOverlays={opts.shapeOverlays}
                            imageOverlays={opts.imageOverlays}
                            chatX={chatX} chatY={chatY} chatW={chatW} chatH={chatH}
                            onChatChange={onChatChange}
                            onShapeChange={handleShapeChange}
                            onImageChange={handleImageChange}
                            onShapeAdd={handleShapeAdd}
                            onShapeRemove={handleShapeRemove}
                            onImageAdd={handleImageAdd}
                            onImageRemove={handleImageRemove}
                            onImageBrowse={handleImageBrowse}
                        />
                    </div>
                )}
            </Section>

            {/* ── Background ─────────────────────────────────────────────────────── */}
            <Section title="Background" defaultOpen={!isBatchItem}>
                <PillTabs options={BG_MODES} value={opts.backgroundMode} onChange={(v) => setOpt("backgroundMode", v)} />
                {opts.backgroundMode === "customColor" && (
                    <ColorPicker label="Background color" value={opts.backgroundColor} onChange={(c) => setOpt("backgroundColor", c)} />
                )}
            </Section>

            {/* ── Typography ─────────────────────────────────────────────────────── */}
            <Section title="Typography" defaultOpen={false}>
                <div className="grid grid-cols-2 gap-2">
                    <div>
                        <div className="flex items-center mb-1"><FieldLabel>Font family</FieldLabel><ResetChip fieldKey="fontName" overriddenKeys={overriddenKeys} onResetKey={onResetKey} /></div>
                        <TextInput value={opts.fontName} onChange={(v) => setOpt("fontName", v)} placeholder="Inter" />
                    </div>
                    <CustomSlider label="Font Size" value={opts.fontSize} min={8} max={72} unit="px" onChange={(v) => setOpt("fontSize", v)} />
                </div>
                <div className="grid grid-cols-2 gap-2">
                    <CustomSlider label="Line Spacing" value={opts.lineSpacing} min={0} max={40} unit="px" onChange={(v) => setOpt("lineSpacing", v)} />
                    <CustomSlider label="Message Spacing" value={opts.messageSpacing} min={0} max={60} unit="px" onChange={(v) => setOpt("messageSpacing", v)} />
                </div>
            </Section>

            {/* ── Layout ─────────────────────────────────────────────────────────── */}
            <Section title="Layout" defaultOpen={false}>
                <div className="grid grid-cols-2 gap-2">
                    <CustomSlider label="Canvas Padding" value={opts.padding} min={0} max={80} unit="px" onChange={(v) => setOpt("padding", v)} />
                    <CustomSlider label="Bubble Padding" value={opts.bubblePadding} min={0} max={40} unit="px" onChange={(v) => setOpt("bubblePadding", v)} />
                </div>
                <CustomSlider label="Bubble Radius" value={opts.bubbleRadius} min={0} max={32} unit="px" onChange={(v) => setOpt("bubbleRadius", v)} />
                <Toggle value={opts.bubbleModeFullWidth} onChange={(v) => setOpt("bubbleModeFullWidth", v)} label="Full-width bubbles" description="Stretch bubbles to canvas edge" />
                <Toggle value={opts.centerEmotesVertically} onChange={(v) => setOpt("centerEmotesVertically", v)} label="Center emotes vertically" />
            </Section>

            {/* ── Colors ─────────────────────────────────────────────────────────── */}
            <Section title="Colors" defaultOpen={false}>
                <ColorPicker label="Message text" value={opts.messageColor} onChange={(c) => setOpt("messageColor", c)} />
                <ColorPicker label="Bubble" value={opts.bubbleColor} onChange={(c) => setOpt("bubbleColor", c)} />
                <ColorPicker label="Highlight" value={opts.highlightColor} onChange={(c) => setOpt("highlightColor", c)} />
            </Section>

            {/* ── Text Style ─────────────────────────────────────────────────────── */}
            <Section title="Text Style" defaultOpen={false}>
                <Toggle value={opts.outlineUsernames} onChange={(v) => setOpt("outlineUsernames", v)} label="Outline usernames" />
                {opts.outlineUsernames && (
                    <CustomSlider label="Outline Width" value={opts.usernameOutlineWidth ?? 1.5} min={0.5} max={6} step={0.5} unit="px" onChange={(v) => setOpt("usernameOutlineWidth", v)} />
                )}
                <Toggle value={opts.usernameShadow} onChange={(v) => setOpt("usernameShadow", v)} label="Username shadow" />
            </Section>

            {/* ── Animations ─────────────────────────────────────────────────────── */}
            <Section title="Animations" defaultOpen={false}>
                <Toggle value={opts.animSlide} onChange={(v) => setOpt("animSlide", v)} label="Slide in" description="Messages slide from the right" />
                <Toggle value={opts.animFadeIn} onChange={(v) => setOpt("animFadeIn", v)} label="Fade in" description="Messages fade from transparent" />
            </Section>

            {/* ── Message Lifecycle ─────────────────────────────────────────────── */}
            <Section title="Message Lifecycle" defaultOpen={false}>
                <SegmentedControl options={EVICTION_MODES} value={opts.evictionStrategy} onChange={(v) => setOpt("evictionStrategy", v)} />
                {opts.evictionStrategy === "timed" && (
                    <div className="grid grid-cols-2 gap-2 pt-1">
                        <CustomSlider label="Hold Duration" value={opts.messageHoldSeconds} min={1} max={60} unit="s" onChange={(v) => setOpt("messageHoldSeconds", v)} />
                        <CustomSlider label="Fade Out" value={opts.messageFadeOutSeconds} min={0} max={10} unit="s" onChange={(v) => setOpt("messageFadeOutSeconds", v)} />
                    </div>
                )}
            </Section>

            {/* ── User Filters ──────────────────────────────────────────────────── */}
            <Section title="User Filters" defaultOpen={false}>
                <div>
                    <FieldLabel>Pinned users (comma-separated)</FieldLabel>
                    <TextInput value={pinnedRaw} onChange={setPinnedRaw} onBlur={() => flush("pinnedUsers", pinnedRaw)} placeholder="streamer, moderator" />
                </div>
                <div>
                    <FieldLabel>Skip users (comma-separated)</FieldLabel>
                    <TextInput value={skipRaw} onChange={setSkipRaw} onBlur={() => flush("skipUsers", skipRaw)} placeholder="BotRix, KickBot" />
                </div>
                <CustomSlider label="Pin Duration" value={opts.pinDurationSecs} min={1} max={60} unit="s" onChange={(v) => setOpt("pinDurationSecs", v)} />
            </Section>

            {/* ── Message Grouping ──────────────────────────────────────────────── */}
            <Section title="Message Grouping" defaultOpen={false}>
                <Toggle value={opts.groupMessages} onChange={(v) => setOpt("groupMessages", v)} label="Group consecutive messages" description="Messages from the same user within the window are merged" />
                {opts.groupMessages && (
                    <CustomSlider label="Grouping Window" value={opts.groupMessagesWindowSecs} min={1} max={30} unit="s" onChange={(v) => setOpt("groupMessagesWindowSecs", v)} />
                )}
            </Section>

            {/* ── Quality ───────────────────────────────────────────────────────── */}
            <Section title="Quality" defaultOpen={false}>
                <SegmentedControl options={QUALITY_PRESETS} value={opts.qualityPreset} onChange={(v) => setOpt("qualityPreset", v)} />
                <div>
                    <FieldLabel>Max cached emotes</FieldLabel>
                    <NumberInput value={opts.maxCachedEmotes} min={16} max={2048} onChange={(v) => setOpt("maxCachedEmotes", v)} />
                </div>
                <Toggle value={opts.createPremultipliedAlphaEmotes} onChange={(v) => setOpt("createPremultipliedAlphaEmotes", v)} label="Premultiplied alpha emotes" description="Faster compositing — disable only if you see fringing" />
            </Section>

            {/* ── Time Window ───────────────────────────────────────────────────── */}
            <Section title="Time Window" defaultOpen={false}>
                <p className="text-[10px] text-neutral-600">Leave blank to render the full log. Values in milliseconds from stream start.</p>
                <div className="grid grid-cols-3 gap-2">
                    <div><FieldLabel>Start (ms)</FieldLabel><NumberInput value={opts.startMs ?? 0} min={0} onChange={(v) => setOpt("startMs", v || undefined)} /></div>
                    <div><FieldLabel>End (ms)</FieldLabel><NumberInput value={opts.endMs ?? 0} min={0} onChange={(v) => setOpt("endMs", v || undefined)} /></div>
                    <div><FieldLabel>Zero point (ms)</FieldLabel><NumberInput value={opts.timeZeroMs ?? 0} min={0} onChange={(v) => setOpt("timeZeroMs", v || undefined)} /></div>
                </div>
            </Section>
        </>
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchItemPanel — redesigned for intuitive inheritance model
// ─────────────────────────────────────────────────────────────────────────────

interface BatchItemPanelProps {
    item: BatchItem;
    index: number;
    mainOpts: RenderVideoArgs;
    allItems: BatchItem[];
    sharedOutputDirectory: string;
    onChange: (updated: BatchItem) => void;
    onRemove: () => void;
    onCopyFrom: (sourceIndex: number) => void;
}

/** A small pill showing inheritance state for a key property */
const InheritBadge = memo(function InheritBadge({ label, isOverridden, onReset }: {
    label: string; isOverridden: boolean; onReset: () => void;
}) {
    if (!isOverridden) {
        return (
            <span className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[9px] bg-neutral-900 text-neutral-600 border border-neutral-800">
                <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                    <path d="M1 4h4M3 2l2 2-2 2" />
                </svg>
                {label}
            </span>
        );
    }
    return (
        <button type="button" onClick={onReset}
                className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[9px] bg-amber-950/50 text-amber-400 border border-amber-800/50 hover:bg-red-950/40 hover:text-red-400 hover:border-red-800/50 transition-colors group">
            <svg width="7" height="7" viewBox="0 0 8 8" fill="currentColor" className="group-hover:hidden">
                <circle cx="4" cy="4" r="2.5" />
            </svg>
            <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" className="hidden group-hover:block">
                <path d="M1.5 1.5l5 5M6.5 1.5l-5 5" />
            </svg>
            {label}
        </button>
    );
});

function BatchItemPanel({ item, index, mainOpts, allItems, sharedOutputDirectory, onChange, onRemove, onCopyFrom }: BatchItemPanelProps) {
    // Batch cards start compact so a 50-item batch remains scannable.
    const [expanded, setExpanded] = useState(false);
    const [showOverrides, setShowOverrides] = useState(false);
    const [showCopyMenu, setShowCopyMenu] = useState(false);
    const copyMenuRef = useRef<HTMLDivElement>(null);

    const effectiveOpts = useMemo(() => resolveItemOpts(mainOpts, item), [mainOpts, item]);
    const { meta: videoMeta, loading: videoLoading} = useVideoMeta(effectiveOpts.useImmediatePipeOverlay ? (item.overlayVideoPath || undefined) : undefined);

    useEffect(() => {
        if (!showCopyMenu) return;
        const onOutsideClick = (e: MouseEvent) => {
            if (copyMenuRef.current && !copyMenuRef.current.contains(e.target as Node))
                setShowCopyMenu(false);
        };
        const onEscape = (e: KeyboardEvent) => {
            if (e.key === "Escape") setShowCopyMenu(false);
        };
        document.addEventListener("mousedown", onOutsideClick);
        document.addEventListener("keydown", onEscape);
        return () => {
            document.removeEventListener("mousedown", onOutsideClick);
            document.removeEventListener("keydown", onEscape);
        };
    }, [showCopyMenu]);

    const overriddenKeys = useMemo(() => new Set(Object.keys(item.overrides)), [item.overrides]);

    const setOverride = useCallback(<K extends keyof RenderVideoArgs>(key: K, value: RenderVideoArgs[K]) => {
        if (BATCH_ITEM_EXCLUSIVE_KEYS.has(key)) {
            onChange({ ...item, [key]: value } as BatchItem);
        } else {
            onChange({ ...item, overrides: { ...item.overrides, [key]: value } });
        }
    }, [item, onChange]);

    const setOverrides = useCallback((patch: Partial<RenderVideoArgs>) => {
        const nextItem = { ...item };
        const nextOverrides = { ...item.overrides };
        for (const [k, v] of Object.entries(patch)) {
            if (BATCH_ITEM_EXCLUSIVE_KEYS.has(k as keyof RenderVideoArgs)) {
                (nextItem as Record<string, unknown>)[k] = v;
            } else {
                (nextOverrides as Record<string, unknown>)[k] = v;
            }
        }
        nextItem.overrides = nextOverrides;
        onChange(nextItem);
    }, [item, onChange]);

    const resetOverride = useCallback((key: string) => {
        const next = { ...item.overrides };
        delete (next as Record<string, unknown>)[key];
        onChange({ ...item, overrides: next });
    }, [item, onChange]);

    const resetAllOverrides = useCallback((e: React.MouseEvent) => {
        e.stopPropagation();
        onChange({ ...item, overrides: {} });
    }, [item, onChange]);

    const handleJsonPathChange = useCallback((v: string) => {
        const nextOutput = item.outputPathOverridden ? item.outputPath : deriveBatchOutputPath(sharedOutputDirectory, v);
        onChange({ ...item, jsonFilePath: v, outputPath: nextOutput, outputPathOverridden: item.outputPathOverridden ?? false });
    }, [item, onChange, sharedOutputDirectory]);
    const handleOutputPathChange = useCallback((v: string) => onChange({ ...item, outputPath: v, outputPathOverridden: true }), [item, onChange]);
    const handleOutputReset = useCallback(() => onChange({ ...item, outputPath: deriveBatchOutputPath(sharedOutputDirectory, item.jsonFilePath), outputPathOverridden: false }), [item, onChange, sharedOutputDirectory]);
    const handleOverlayPathChange = useCallback((v: string) => onChange({ ...item, overlayVideoPath: v }), [item, onChange]);
    const handleClearOverlayPath = useCallback(() => onChange({ ...item, overlayVideoPath: "" }), [item, onChange]);
    const handlePipeModeToggle = useCallback((v: boolean) => setOverride("useImmediatePipeOverlay", v), [setOverride]);

    const hasOverrides = overriddenKeys.size > 0;
    const missingPipeVideo = effectiveOpts.useImmediatePipeOverlay && !item.overlayVideoPath;
    const canDispatch = Boolean(item.jsonFilePath)
        && Boolean(item.outputPath || deriveBatchOutputPath(sharedOutputDirectory, item.jsonFilePath))
        && !missingPipeVideo;

    // Copy targets: "Main settings" first, then sibling items
    const copyTargets = useMemo(() => {
        const siblings = allItems
            .map((_it, i) => ({ kind: "item" as const, index: i, label: `Item ${i + 1}` }))
            .filter((t) => t.index !== index);
        return [{ kind: "main" as const, index: -1, label: "Main settings" }, ...siblings];
    }, [allItems, index]);

    const handleSelectJsonl = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "JSONL", extensions: ["jsonl"] }] });
            if (typeof sel === "string") {
                const nextOutput = item.outputPathOverridden ? item.outputPath : deriveBatchOutputPath(sharedOutputDirectory, sel);
                onChange({ ...item, jsonFilePath: sel, outputPath: nextOutput, outputPathOverridden: item.outputPathOverridden ?? false });
            }
        } catch {}
    }, [item, onChange, sharedOutputDirectory]);

    const handleSelectOutput = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, directory: true });
            if (typeof sel === "string") {
                const path = deriveBatchOutputPath(sel, item.jsonFilePath) || joinPath(sel, `output_${index + 1}.mp4`);
                onChange({ ...item, outputPath: path, outputPathOverridden: true });
            }
        } catch {}
    }, [item, index, onChange]);

    const handleSelectOverlayVideo = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "Video", extensions: ["mp4", "mkv", "mov", "avi", "webm"] }] });
            if (typeof sel === "string") onChange({ ...item, overlayVideoPath: sel });
        } catch {}
    }, [item, onChange]);

    const isOverlayMode = effectiveOpts.useImmediatePipeOverlay;
    const bgWidth = (isOverlayMode && videoMeta) ? videoMeta.width : effectiveOpts.width;
    const bgHeight = (isOverlayMode && videoMeta) ? videoMeta.height : effectiveOpts.height;

    const handleItemShapeAdd = useCallback(() =>
            setOverride("shapeOverlays", [...effectiveOpts.shapeOverlays, { x: 20, y: 20, width: Math.round(effectiveOpts.width / 3), height: Math.round(effectiveOpts.height / 6), color: { alpha: 200, red: 30, green: 30, blue: 30 }, cornerRadius: 8 }]),
        [effectiveOpts, setOverride]);

    const handleItemShapeRemove = useCallback((i: number) =>
            setOverride("shapeOverlays", effectiveOpts.shapeOverlays.filter((_, idx) => idx !== i)),
        [effectiveOpts.shapeOverlays, setOverride]);

    const handleItemImageAdd = useCallback(() =>
            setOverride("imageOverlays", [...effectiveOpts.imageOverlays, { assetPath: "", x: 0, y: 0, width: Math.round(effectiveOpts.width / 3), height: Math.round(effectiveOpts.height / 5), alpha: 1.0 }]),
        [effectiveOpts, setOverride]);

    const handleItemImageRemove = useCallback((i: number) =>
            setOverride("imageOverlays", effectiveOpts.imageOverlays.filter((_, idx) => idx !== i)),
        [effectiveOpts.imageOverlays, setOverride]);

    const handleItemShapeChange = useCallback((i: number, patch: Partial<CustomShapeOverlay>) => {
        const next = [...effectiveOpts.shapeOverlays]; next[i] = { ...next[i], ...patch };
        setOverride("shapeOverlays", next);
    }, [effectiveOpts.shapeOverlays, setOverride]);

    const handleItemImageChange = useCallback((i: number, patch: Partial<CustomImageOverlay>) => {
        const next = [...effectiveOpts.imageOverlays]; next[i] = { ...next[i], ...patch };
        setOverride("imageOverlays", next);
    }, [effectiveOpts.imageOverlays, setOverride]);

    const handleItemChatChange = useCallback((x: number, y: number, w: number, h: number) =>
            setOverrides({ overlayX: x, overlayY: y, overlayWidth: w, overlayHeight: h }),
        [setOverrides]);

    const handleItemImageBrowse = useCallback(async (i: number) => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }] });
            if (typeof sel === "string") {
                const next = [...effectiveOpts.imageOverlays]; next[i] = { ...next[i], assetPath: sel };
                setOverride("imageOverlays", next);
            }
        } catch {}
    }, [effectiveOpts.imageOverlays, setOverride]);

    const sourceFileName = item.jsonFilePath ? getPathFileName(item.jsonFilePath) : null;
    const effectiveOutputPath = item.outputPath || deriveBatchOutputPath(sharedOutputDirectory, item.jsonFilePath);
    const outputFileName = effectiveOutputPath ? getPathFileName(effectiveOutputPath) : null;
    const outputUsesTemplate = !item.outputPathOverridden;

    return (
        <div className={`rounded-xl border overflow-hidden transition-all duration-200 ${
            canDispatch
                ? "border-neutral-700/70 shadow-sm shadow-black/20"
                : "border-neutral-800/80"
        }`}>
            {/* ── Card Header ───────────────────────────────────────────────── */}
            <div className={`flex items-center gap-0 transition-colors ${
                canDispatch ? "bg-neutral-900/80" : "bg-neutral-900/50"
            }`}>
                {/* Ready indicator strip */}
                <div className={`w-1 self-stretch shrink-0 rounded-l-xl transition-colors ${
                    canDispatch ? "bg-violet-600/60" : "bg-neutral-800"
                }`} />

                <div className="flex-1 flex items-center gap-2.5 px-3 py-2.5 min-w-0">
                    {/* Collapse toggle */}
                    <button type="button" onClick={() => setExpanded((v) => !v)}
                            className="flex items-center gap-1.5 shrink-0 group">
                        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"
                             className={`text-neutral-600 group-hover:text-neutral-400 transition-all duration-200 ${expanded ? "rotate-0" : "-rotate-90"}`}>
                            <path d="M2 3.5l3 3 3-3" />
                        </svg>
                    </button>

                    {/* Zone label — always visible */}
                    <span className="flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded bg-neutral-800 border border-neutral-700 text-neutral-400 font-bold uppercase tracking-wider shrink-0">
                        <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                            <rect x="1" y="1" width="6" height="6" rx="1" />
                        </svg>
                        Item {index + 1}
                    </span>

                    {/* Source → Output summary */}
                    <div className="flex-1 flex items-center gap-1.5 min-w-0 overflow-hidden">
                        {sourceFileName ? (
                            <span className="text-[10px] font-mono text-neutral-300 truncate">{sourceFileName}</span>
                        ) : (
                            <span className="text-[10px] text-red-500/70 italic">no source set</span>
                        )}
                        {canDispatch && (
                            <>
                                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" className="text-neutral-700 shrink-0">
                                    <path d="M2 5h6M5 2l3 3-3 3" />
                                </svg>
                                <span className="text-[10px] font-mono text-neutral-500 truncate">{outputFileName}</span>
                            </>
                        )}
                    </div>

                    {/* Status badges */}
                    <div className="flex items-center gap-1.5 shrink-0">
                        {canDispatch ? (
                            <span className="flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded-full bg-emerald-950/60 text-emerald-400 border border-emerald-800/50 font-medium">
                                <span className="w-1 h-1 rounded-full bg-emerald-400" />
                                ready
                            </span>
                        ) : (
                            <span className={`flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded-full border ${missingPipeVideo ? "bg-orange-950/40 text-orange-400 border-orange-800/50" : "bg-neutral-800 text-neutral-600 border-neutral-700/50"}`}>
                                {missingPipeVideo ? "needs base video" : "incomplete"}
                            </span>
                        )}
                        {outputUsesTemplate ? (
                            <span className="text-[9px] px-1.5 py-0.5 rounded-full bg-violet-950/50 text-violet-300 border border-violet-800/50 font-medium">
                                template output
                            </span>
                        ) : (
                            <span className="text-[9px] px-1.5 py-0.5 rounded-full bg-amber-950/60 text-amber-400 border border-amber-800/50 font-medium">
                                output override
                            </span>
                        )}
                        {hasOverrides && (
                            <span className="text-[9px] px-1.5 py-0.5 rounded-full bg-amber-950/60 text-amber-400 border border-amber-800/50 font-medium">
                                {overriddenKeys.size} override{overriddenKeys.size !== 1 ? "s" : ""}
                            </span>
                        )}
                    </div>

                    {/* Actions */}
                    <div className="flex items-center gap-0.5 shrink-0">
                        {/* Copy from dropdown */}
                        <div className="relative" ref={copyMenuRef}>
                            <button type="button" onClick={() => setShowCopyMenu((v) => !v)}
                                    title="Copy settings from template or another item"
                                    className="flex items-center gap-1 px-2 py-1.5 rounded text-[10px] text-neutral-500 hover:text-violet-400 hover:bg-violet-950/30 transition-colors">
                                <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                                    <rect x="4" y="4" width="9" height="11" rx="1.5" />
                                    <path d="M4 4V3a1 1 0 0 1 1-1h8a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-1" />
                                </svg>
                                <span className="hidden sm:inline">Copy from</span>
                            </button>
                            {showCopyMenu && (
                                <div className="absolute right-0 top-full mt-1 bg-neutral-950 border border-neutral-700 rounded-xl shadow-2xl z-50 overflow-hidden min-w-52">
                                    <div className="px-3 py-2 text-[9px] text-neutral-600 uppercase tracking-widest font-bold border-b border-neutral-800 bg-neutral-900/60">
                                        Copy all settings from
                                    </div>
                                    {copyTargets.map((t) => (
                                        <button key={`${t.kind}-${t.index}`} type="button"
                                                onClick={() => {
                                                    if (t.kind === "main") {
                                                        onChange({ ...item, overrides: {} });
                                                    } else {
                                                        onCopyFrom(t.index);
                                                    }
                                                    setShowCopyMenu(false);
                                                }}
                                                className="w-full px-3 py-2.5 text-left text-xs hover:bg-neutral-800/80 transition-colors border-b border-neutral-800/50 last:border-0">
                                            <div className="flex items-center gap-2">
                                                {t.kind === "main" ? (
                                                    <span className="w-5 h-5 rounded bg-violet-600/20 border border-violet-500/40 flex items-center justify-center shrink-0">
                                                        <svg width="8" height="8" viewBox="0 0 8 8" fill="currentColor" className="text-violet-400">
                                                            <circle cx="4" cy="4" r="2.5" />
                                                        </svg>
                                                    </span>
                                                ) : (
                                                    <span className="w-5 h-5 rounded bg-neutral-800 border border-neutral-700 flex items-center justify-center shrink-0 text-[8px] text-neutral-400 font-bold">
                                                        {t.index + 1}
                                                    </span>
                                                )}
                                                <div className="min-w-0">
                                                    <div className={`font-semibold text-[11px] ${t.kind === "main" ? "text-violet-300" : "text-neutral-300"}`}>
                                                        {t.label}
                                                    </div>
                                                    {t.kind === "main" && (
                                                        <div className="text-[9px] text-neutral-600">Clears all overrides on this item</div>
                                                    )}
                                                    {t.kind === "item" && allItems[t.index]?.jsonFilePath && (
                                                        <div className="text-[9px] text-neutral-600 font-mono truncate">
                                                            {allItems[t.index].jsonFilePath.split(/[/\\]/).pop()}
                                                        </div>
                                                    )}
                                                </div>
                                            </div>
                                        </button>
                                    ))}
                                </div>
                            )}
                        </div>

                        <button type="button" onClick={onRemove} title="Remove item"
                                className="w-6 h-6 flex items-center justify-center text-neutral-700 hover:text-red-400 rounded hover:bg-red-950/20 transition-colors">
                            <svg width="9" height="9" viewBox="0 0 8 8" fill="none"><path d="M1 1l6 6M7 1L1 7" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" /></svg>
                        </button>
                    </div>
                </div>
            </div>

            {/* ── Card Body (collapsible) ────────────────────────────────────── */}
            {expanded && (
                <div className="border-t border-neutral-800/60 bg-neutral-950/30">
                    {/* ── Required fields ─────────────────────────────────────────── */}
                    <div className="px-4 pt-3.5 pb-3 space-y-2.5">
                        <div className="flex items-center gap-2 mb-2">
                            <span className="flex items-center gap-1 text-[9px] font-bold text-neutral-400 uppercase tracking-widest">
                                <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                                    <path d="M4 1v6M1 4h6" />
                                </svg>
                                Item {index + 1} — required
                            </span>
                            <div className="flex-1 border-t border-neutral-800/80" />
                            <span className="text-[9px] text-neutral-700">unique per item</span>
                        </div>

                        <div>
                            <FieldLabel>Chat source (.jsonl)</FieldLabel>
                            <BrowseInput value={item.jsonFilePath} onChange={handleJsonPathChange}
                                         onBrowse={handleSelectJsonl} placeholder="/path/to/chat.jsonl" browseLabel="Browse" />
                        </div>
                        <div>
                            <div className="flex items-center justify-between mb-1">
                                <div className="flex items-center gap-1.5">
                                    <FieldLabel>Output path</FieldLabel>
                                    <span className={`text-[9px] px-1.5 py-0.5 rounded border ${outputUsesTemplate ? "text-violet-300 bg-violet-950/40 border-violet-800/40" : "text-amber-300 bg-amber-950/40 border-amber-800/40"}`}>
                                        {outputUsesTemplate ? "shared folder" : "override"}
                                    </span>
                                </div>
                                {!outputUsesTemplate && (
                                    <button type="button" onClick={handleOutputReset} className="text-[9px] text-neutral-600 hover:text-violet-400 transition-colors">
                                        Use template folder
                                    </button>
                                )}
                            </div>
                            <BrowseInput value={effectiveOutputPath} onChange={handleOutputPathChange}
                                         onBrowse={handleSelectOutput} placeholder="/path/to/output.mp4" browseLabel="Browse" />
                        </div>
                    </div>

                    {/* ── Video overlay (item-exclusive) ───────────────────────────── */}
                    <div className="px-4 pb-3 pt-2 border-t border-neutral-800/50 space-y-2.5">
                        <div className="flex items-center gap-2 mb-2">
                            <span className="flex items-center gap-1 text-[9px] font-bold text-neutral-500 uppercase tracking-widest">
                                <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                                    <rect x="1" y="1" width="6" height="6" rx="1" />
                                </svg>
                                Item {index + 1} — video source
                            </span>
                            <div className="flex-1 border-t border-neutral-800/80" />
                            <span className="text-[9px] text-orange-500/70">not inherited from template</span>
                        </div>
                        <div className="flex items-start gap-2">
                            <div className="flex-1">
                                <Toggle value={effectiveOpts.useImmediatePipeOverlay} onChange={handlePipeModeToggle}
                                        label="Direct Pipe Mode (Video Overlay)"
                                        description="The toggle inherits from the shared template; the base video is always item-specific." />
                            </div>
                            {!overriddenKeys.has("useImmediatePipeOverlay") ? (
                                <span className="mt-0.5 text-[9px] px-1.5 py-0.5 rounded border text-violet-300 bg-violet-950/40 border-violet-800/40">Template</span>
                            ) : (
                                <button type="button" onClick={() => resetOverride("useImmediatePipeOverlay")} className="mt-0.5 text-[9px] px-1.5 py-0.5 rounded border text-amber-300 bg-amber-950/40 border-amber-800/40 hover:text-red-300 hover:border-red-800/50 transition-colors">
                                    Override · reset
                                </button>
                            )}
                        </div>
                        {effectiveOpts.useImmediatePipeOverlay && (
                            <div>
                                <FieldLabel>Base video</FieldLabel>
                                <BrowseInput value={item.overlayVideoPath || ""} onChange={handleOverlayPathChange}
                                             onBrowse={handleSelectOverlayVideo} onClear={handleClearOverlayPath}
                                             placeholder="/path/to/stream.mp4" browseLabel="Browse" />
                            </div>
                        )}
                    </div>

                    {/* ── Canvas editor (when pipe mode + video set) ─────────────── */}
                    {effectiveOpts.useImmediatePipeOverlay && !!effectiveOpts.overlayVideoPath && (
                        <div className="px-4 pb-3 pt-2 border-t border-neutral-800/50">
                            <div className="flex items-center justify-between gap-3 mb-2">
                                <p className="text-[10px] text-neutral-600">
                                    Item canvas · geometry and styles come from the template until you change them here.
                                </p>
                                <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded border border-violet-800/40 bg-violet-950/30 text-violet-300">base video: item</span>
                            </div>
                            {videoLoading ? (
                                <div className="w-full h-28 rounded-lg bg-neutral-900/60 border border-neutral-700/50 flex flex-col items-center justify-center gap-2 text-neutral-500">
                                    <span className="w-4 h-4 border-2 border-neutral-700 border-t-indigo-500 rounded-full animate-spin" />
                                    <span className="text-[10px]">Loading video…</span>
                                </div>
                            ) : (
                                <CanvasEditor
                                    bgWidth={bgWidth}
                                    bgHeight={bgHeight}
                                    videoMeta={videoMeta ?? null}
                                    isOverlayMode={isOverlayMode}
                                    shapeOverlays={effectiveOpts.shapeOverlays}
                                    imageOverlays={effectiveOpts.imageOverlays}
                                    chatX={effectiveOpts.overlayX ?? 0}
                                    chatY={effectiveOpts.overlayY ?? 0}
                                    chatW={effectiveOpts.overlayWidth ?? effectiveOpts.width}
                                    chatH={effectiveOpts.overlayHeight ?? effectiveOpts.height}
                                    onChatChange={handleItemChatChange}
                                    onShapeChange={handleItemShapeChange}
                                    onImageChange={handleItemImageChange}
                                    onShapeAdd={handleItemShapeAdd}
                                    onShapeRemove={handleItemShapeRemove}
                                    onImageAdd={handleItemImageAdd}
                                    onImageRemove={handleItemImageRemove}
                                    onImageBrowse={handleItemImageBrowse}
                                />
                            )}
                        </div>
                    )}


                    {/* ── Render settings override zone ────────────────────────────── */}
                    <div className="border-t border-neutral-800/50">
                        <div className="w-full flex items-center gap-2 px-4 py-2.5 hover:bg-neutral-900/40 transition-colors">
                            <button type="button" onClick={() => setShowOverrides((v) => !v)}
                                    className="min-w-0 flex-1 flex items-center gap-2 text-left group">
                                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"
                                     className={`text-neutral-600 group-hover:text-neutral-400 transition-transform duration-150 shrink-0 ${showOverrides ? "rotate-0" : "-rotate-90"}`}>
                                    <path d="M2 3.5l3 3 3-3" />
                                </svg>
                                {!hasOverrides ? (
                                    <span className="flex items-center gap-1.5 text-[9px] font-bold uppercase tracking-wider text-neutral-600">
                                        <span className="px-1.5 py-0.5 rounded bg-violet-950/60 text-violet-400/70 border border-violet-800/40">Template</span>
                                        Render settings — all from shared template
                                    </span>
                                ) : (
                                    <span className="flex items-center gap-1.5 text-[9px] font-bold uppercase tracking-wider text-amber-400">
                                        <span className="px-1.5 py-0.5 rounded bg-neutral-800 border border-neutral-700 text-neutral-400">Item {index + 1}</span>
                                        {overriddenKeys.size} override{overriddenKeys.size !== 1 ? "s" : ""} — rest from template
                                    </span>
                                )}
                            </button>
                            {hasOverrides && (
                                <button type="button" onClick={resetAllOverrides}
                                        className="shrink-0 text-[9px] text-neutral-600 hover:text-red-400 transition-colors px-1.5 py-0.5 rounded hover:bg-red-950/20 font-normal normal-case tracking-normal">
                                    ↺ Reset all to template
                                </button>
                            )}
                        </div>

                        {/* Collapsed: show override pills */}
                        {!showOverrides && hasOverrides && (
                            <div className="flex flex-wrap gap-1 px-4 pb-3">
                                {Array.from(overriddenKeys).map((k) => (
                                    <InheritBadge key={k} label={k} isOverridden onReset={() => resetOverride(k)} />
                                ))}
                            </div>
                        )}

                        {/* Collapsed + clean: brief confirmation */}
                        {!showOverrides && !hasOverrides && (
                            <p className="text-[10px] text-neutral-700 px-4 pb-3">
                                Canvas, background, typography, colors, quality — all from the shared template above.
                            </p>
                        )}

                        {/* Expanded: full settings panel with edit context banner */}
                        {showOverrides && (
                            <div className="border-t border-neutral-800/50">
                                {/* Sticky context banner so you never forget what you're editing */}
                                <div className="flex items-center gap-2 px-4 py-2 bg-neutral-900 border-b border-neutral-800 sticky top-0 z-10">
                                    <span className="text-[9px] font-bold uppercase tracking-widest text-neutral-600">Editing</span>
                                    <span className="flex items-center gap-1 text-[9px] px-2 py-0.5 rounded bg-neutral-800 border border-neutral-700 text-neutral-300 font-bold">
                                        <svg width="7" height="7" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                                            <rect x="1" y="1" width="6" height="6" rx="1" />
                                        </svg>
                                        Item {index + 1} overrides
                                    </span>
                                    <span className="text-[9px] text-neutral-700">
                                        — changes here only affect this item
                                    </span>
                                    <div className="ml-auto flex items-center gap-1">
                                        <span className="text-[9px] text-neutral-700">Base:</span>
                                        <span className="flex items-center gap-1 text-[9px] px-1.5 py-0.5 rounded bg-violet-950/50 border border-violet-800/40 text-violet-400 font-bold">
                                            <svg width="6" height="6" viewBox="0 0 8 8" fill="currentColor" className="text-violet-400">
                                                <circle cx="4" cy="4" r="3" />
                                            </svg>
                                            Shared template
                                        </span>
                                    </div>
                                </div>
                                <div className="px-4 py-4 space-y-4">
                                    <SettingsPanel
                                        opts={effectiveOpts}
                                        setOpt={setOverride}
                                        setOpts={setOverrides}
                                        overriddenKeys={overriddenKeys}
                                        onResetKey={resetOverride}
                                        isBatchItem={true}
                                    />
                                </div>
                            </div>
                        )}
                    </div>
                </div>
            )}
        </div>
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Main RenderForm
// ─────────────────────────────────────────────────────────────────────────────

export function RenderForm({ tab, onUpdate }: RenderFormProps) {
    const { registerTaskSnapshot } = useWorkspace();
    const navigate = useNavigate();

    const opts: RenderVideoArgs = useMemo(
        () => ({ ...RENDER_DEFAULTS, ...(tab.renderOptions ?? {}) }),
        [tab.renderOptions],
    );

    const [mode, setMode] = useState<"single" | "batch">("single");
    const [batchItems, setBatchItems] = useState<BatchItem[]>(() => [makeBatchItem()]);
    const [batchOutputDirectory, setBatchOutputDirectoryState] = useState("");
    const [batchPattern, setBatchPattern] = useState("*-cut*");
    const [batchMatchMode, setBatchMatchMode] = useState<BatchMatchMode>("glob");
    const [batchImportError, setBatchImportError] = useState<string | null>(null);
    const [batchImportNotice, setBatchImportNotice] = useState<string | null>(null);
    const [batchImporting, setBatchImporting] = useState(false);
    // Batch template reference video is UI-only. It deliberately never enters
    // RenderVideoArgs or the backend payload; each item owns its actual base video.
    const [templatePreviewVideoPath, setTemplatePreviewVideoPath] = useState("");

    const addBatchItem = useCallback(() => setBatchItems((prev) => [...prev, makeBatchItem()]), []);

    const updateBatchItem = useCallback((id: string, updated: BatchItem) =>
            setBatchItems((prev) => prev.map((it) => it.id === id ? updated : it)),
        []);

    const removeBatchItem = useCallback((id: string) =>
            setBatchItems((prev) => {
                const next = prev.filter((it) => it.id !== id);
                return next.length === 0 ? [makeBatchItem()] : next;
            }),
        []);

    const copySettingsFromItem = useCallback((targetId: string, sourceIndex: number) => {
        setBatchItems((prev) => {
            const src = prev[sourceIndex];
            if (!src) return prev;
            // Resolve against current opts inside the setter so we don't close over stale opts.
            const copyable = extractCopyableSettings(resolveItemOpts(opts, src));
            return prev.map((it) => it.id === targetId ? { ...it, overrides: copyable } : it);
        });
    }, [opts]);

    const setBatchOutputDirectory = useCallback((directory: string) => {
        setBatchOutputDirectoryState(directory);
        setBatchItems((prev) => prev.map((item) => item.outputPathOverridden
            ? item
            : { ...item, outputPath: deriveBatchOutputPath(directory, item.jsonFilePath) }
        ));
    }, []);

    const appendImportedPairs = useCallback((pairs: BatchPair[]) => {
        if (pairs.length === 0) return;
        const destination = batchOutputDirectory;
        setBatchItems((prev) => {
            const blankOnly = prev.length === 1 && !prev[0].jsonFilePath;
            const base = blankOnly ? [] : prev;
            const existingJson = new Set(base.map((item) => item.jsonFilePath.toLowerCase()));
            const imported = pairs
                .filter((pair) => !existingJson.has(pair.jsonFilePath.toLowerCase()))
                .map((pair) => ({
                    ...makeBatchItem(),
                    jsonFilePath: pair.jsonFilePath,
                    overlayVideoPath: pair.videoFilePath ?? "",
                    outputPath: deriveBatchOutputPath(destination, pair.jsonFilePath),
                }));
            return [...base, ...imported];
        });

        if (!batchOutputDirectory && pairs[0]?.jsonFilePath) {
            // Keep the template folder blank; each imported item derives its own source-folder output.
            setBatchOutputDirectoryState("");
        }
        setBatchImportNotice(`Imported ${pairs.length} JSONL item${pairs.length === 1 ? "" : "s"}${pairs.some((p) => p.videoFilePath) ? " with matching video references" : ""}.`);
        setBatchImportError(null);
    }, [batchOutputDirectory]);

    const scanSelectedBatchFiles = useCallback(async () => {
        setBatchImporting(true);
        setBatchImportError(null);
        setBatchImportNotice(null);
        try {
            const selection = await open({
                multiple: true,
                filters: [{ name: "Batch files", extensions: ["jsonl", "mp4", "mkv", "mov", "avi", "webm"] }],
            });
            const paths = Array.isArray(selection) ? selection : (typeof selection === "string" ? [selection] : []);
            const pairs = pairBatchFiles(paths, batchPattern, batchMatchMode);
            if (pairs.length === 0) {
                throw new Error("No matching JSONL files were found. Add the matching video files too when Direct Pipe mode needs per-item base videos.");
            }
            appendImportedPairs(pairs);
        } catch (e) {
            if (e instanceof Error && e.message.includes("No matching")) setBatchImportError(e.message);
            else if (e) setBatchImportError(e instanceof Error ? e.message : String(e));
        } finally {
            setBatchImporting(false);
        }
    }, [appendImportedPairs, batchMatchMode, batchPattern]);

    const scanBatchFolder = useCallback(async () => {
        setBatchImporting(true);
        setBatchImportError(null);
        setBatchImportNotice(null);
        try {
            const selection = await open({ multiple: false, directory: true });
            if (typeof selection !== "string") return;
            setBatchOutputDirectory(selection);

            // Folder enumeration lives in the native layer because the browser sandbox
            // cannot reliably turn a selected Tauri directory into absolute file paths.
            // The frontend accepts the result as either string paths or { path } entries.
            const entries = await invoke<unknown[]>("read_directory_files", { path: selection });
            const paths = entries.flatMap((entry) => {
                if (typeof entry === "string") return [entry];
                if (entry && typeof entry === "object" && "path" in entry && typeof (entry as { path?: unknown }).path === "string") return [(entry as { path: string }).path];
                return [];
            });
            const pairs = pairBatchFiles(paths, batchPattern, batchMatchMode);
            if (pairs.length === 0) {
                throw new Error("The selected folder contains no matching JSONL files for the current pattern.");
            }
            appendImportedPairs(pairs);
        } catch (e) {
            setBatchImportError(
                e instanceof Error
                    ? `${e.message} — group-of-files import is available as a fallback if your backend does not expose folder enumeration yet.`
                    : String(e),
            );
        } finally {
            setBatchImporting(false);
        }
    }, [appendImportedPairs, batchMatchMode, batchPattern, setBatchOutputDirectory]);

    const setOpt = useCallback(<K extends keyof RenderVideoArgs>(key: K, value: RenderVideoArgs[K]) => {
        onUpdate({ renderOptions: { ...opts, [key]: value } });
    }, [onUpdate, opts]);

    const setOpts = useCallback((patch: Partial<RenderVideoArgs>) => {
        onUpdate({ renderOptions: { ...opts, ...patch } });
    }, [onUpdate, opts]);

    const { meta: videoMeta, loading: videoLoading, error: videoError } = useVideoMeta(opts.useImmediatePipeOverlay ? opts.overlayVideoPath : undefined);
    const { meta: templatePreviewVideoMeta, loading: templatePreviewVideoLoading, error: templatePreviewVideoError } =
        useVideoMeta(opts.useImmediatePipeOverlay ? (templatePreviewVideoPath || undefined) : undefined);

    const [isDispatching, setIsDispatching] = useState(false);
    const [dispatchError, setDispatchError] = useState<string | null>(null);

    const handleSelectJsonl = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "JSONL", extensions: ["jsonl"] }] });
            if (typeof sel === "string") onUpdate({ jsonFilePath: sel });
        } catch {}
    }, [onUpdate]);

    const handleSelectOutput = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, directory: true });
            if (typeof sel === "string") {
                // Derive separator from the path rather than guessing platform —
                // backslashes are valid in macOS filenames so sniffing is unreliable.
                const sep = sel.includes("\\") && !sel.includes("/") ? "\\" : "/";
                setOpt("outputPath", sel.endsWith(sep) ? `${sel}output.mp4` : `${sel}${sep}output.mp4`);
            }
        } catch {}
    }, [setOpt]);

    const handleSelectOverlayVideo = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "Video", extensions: ["mp4", "mkv", "mov", "avi", "webm"] }] });
            if (typeof sel === "string") setOpt("overlayVideoPath", sel);
        } catch {}
    }, [setOpt]);

    const handleSetOverlayVideoPath = useCallback((v: string) => setOpt("overlayVideoPath", v || undefined), [setOpt]);

    const handleSelectTemplatePreviewVideo = useCallback(async () => {
        try {
            const sel = await open({ multiple: false, filters: [{ name: "Video", extensions: ["mp4", "mkv", "mov", "avi", "webm"] }] });
            if (typeof sel === "string") setTemplatePreviewVideoPath(sel);
        } catch {}
    }, []);

    const handleDispatch = useCallback(async () => {
        if (!tab.jsonFilePath || !opts.outputPath || isDispatching) return;
        setIsDispatching(true); setDispatchError(null);
        const taskId = crypto.randomUUID();
        try {
            await invoke("queue_chat_render", { id: taskId, jsonFilePath: tab.jsonFilePath, options: opts });
            registerTaskSnapshot({ tabId: tab.id, taskId, url: tab.url, jsonFilePath: tab.jsonFilePath, vodOptions: tab.vodOptions, chatOptions: tab.chatOptions, renderOptions: opts });
            onUpdate({ activeTaskId: taskId });
            navigate({ to: "/queue" });
        } catch (e) { setDispatchError(e instanceof Error ? e.message : String(e)); }
        finally { setIsDispatching(false); }
    }, [tab, opts, isDispatching, navigate, onUpdate, registerTaskSnapshot]);

    const missingSinglePipeVideo = opts.useImmediatePipeOverlay && !opts.overlayVideoPath;
    const canDispatch = Boolean(tab.jsonFilePath) && Boolean(opts.outputPath) && !missingSinglePipeVideo && !isDispatching;

    const [batchDispatching, setBatchDispatching] = useState(false);
    const [batchDispatchError, setBatchDispatchError] = useState<string | null>(null);

    const getBatchItemReadiness = useCallback((item: BatchItem) => {
        if (!item.jsonFilePath) return { ready: false, reason: "Missing JSONL source" };
        const outputPath = item.outputPath || deriveBatchOutputPath(batchOutputDirectory, item.jsonFilePath);
        if (!outputPath) return { ready: false, reason: "Missing output path" };

        const effective = resolveItemOpts(opts, item);
        if (effective.useImmediatePipeOverlay && !item.overlayVideoPath) {
            return { ready: false, reason: "Direct Pipe requires an item base video" };
        }
        return { ready: true, reason: "Ready" };
    }, [batchOutputDirectory, opts]);

    const readyBatchItems = useMemo(
        () => batchItems.filter((it) => getBatchItemReadiness(it).ready),
        [batchItems, getBatchItemReadiness],
    );

    const handleBatchDispatch = useCallback(async () => {
        if (readyBatchItems.length === 0 || batchDispatching) return;
        setBatchDispatching(true);
        setBatchDispatchError(null);
        try {
            // Build the payload for the single atomic backend call.
            // Each item gets its effective opts resolved against main before sending.
            const batchPayload = readyBatchItems.map((item) => ({
                id: crypto.randomUUID(),
                jsonFilePath: item.jsonFilePath,
                options: resolveItemOpts(opts, {
                    ...item,
                    outputPath: item.outputPath || deriveBatchOutputPath(batchOutputDirectory, item.jsonFilePath),
                }),
            }));

            // Single invoke — backend handles semaphore throttling and ordering.
            // Returns the task IDs in the same order as the input items.
            const taskIds = await invoke<string[]>("queue_batch_chat_render", {
                items: batchPayload,
            });
            if (!Array.isArray(taskIds) || taskIds.length !== batchPayload.length || taskIds.some((id) => !id)) {
                throw new Error(`Batch queue returned ${Array.isArray(taskIds) ? taskIds.length : 0} task IDs for ${batchPayload.length} requested renders.`);
            }

            // Register snapshots so "Return to workspace" works from QueueRow.
            taskIds.forEach((taskId, i) => {
                const item = batchPayload[i];
                if (!item) return;
                registerTaskSnapshot({
                    tabId: tab.id,
                    taskId,
                    url: tab.url,
                    jsonFilePath: item.jsonFilePath,
                    vodOptions: tab.vodOptions,
                    chatOptions: tab.chatOptions,
                    renderOptions: item.options,
                    downloadMode: tab.downloadMode,
                    metadata: tab.metadata,
                });
            });

            // Track the last task ID on the tab (mirrors single-render behaviour)
            const lastId = taskIds[taskIds.length - 1];
            if (lastId) onUpdate({ activeTaskId: lastId });

            navigate({ to: "/queue" });
        } catch (e) {
            setBatchDispatchError(e instanceof Error ? e.message : String(e));
        } finally {
            setBatchDispatching(false);
        }
    }, [readyBatchItems, batchDispatching, opts, tab, navigate, onUpdate, registerTaskSnapshot, batchOutputDirectory]);

    return (
        <div className="space-y-5 pb-4">
            {/* ── Mode toggle ──────────────────────────────────────────────────── */}
            <div className="flex items-center gap-1 p-0.5 bg-neutral-900 border border-neutral-800 rounded-lg w-fit">
                {(["single", "batch"] as const).map((m) => (
                    <button key={m} type="button" onClick={() => setMode(m)}
                            className={`px-3 py-1.5 text-xs font-semibold rounded-md transition-all ${
                                mode === m
                                    ? "bg-violet-600 text-white shadow-sm shadow-violet-900/50"
                                    : "text-neutral-500 hover:text-neutral-300"
                            }`}>
                        {m === "single" ? "Single render" : "Batch render"}
                    </button>
                ))}
            </div>

            {/* ── SINGLE ───────────────────────────────────────────────────────── */}
            {mode === "single" && (
                <>
                    <Section title="Source & Output">
                        <div>
                            <FieldLabel>Chat source (.jsonl)</FieldLabel>
                            <BrowseInput value={tab.jsonFilePath ?? ""} onChange={(v) => onUpdate({ jsonFilePath: v })}
                                         onBrowse={handleSelectJsonl} placeholder="/path/to/chat.jsonl" browseLabel="Browse" />
                        </div>
                        <div>
                            <FieldLabel>Output path</FieldLabel>
                            <BrowseInput value={opts.outputPath} onChange={(v) => setOpt("outputPath", v)}
                                         onBrowse={handleSelectOutput} placeholder="/path/to/output.mp4" browseLabel="Browse" />
                        </div>
                    </Section>

                    <SettingsPanel
                        opts={opts} setOpt={setOpt} setOpts={setOpts}
                        overlayVideoPath={opts.overlayVideoPath}
                        onSelectOverlayVideo={handleSelectOverlayVideo}
                        onSetOverlayVideoPath={handleSetOverlayVideoPath}
                        videoMeta={videoMeta}
                        videoLoading={videoLoading}
                        videoError={videoError}
                    />

                    {missingSinglePipeVideo && (
                        <div className="flex items-start gap-2 px-3 py-2.5 bg-orange-950/30 border border-orange-800/40 rounded-lg text-xs text-orange-300">
                            <svg className="shrink-0 mt-0.5" width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                                <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" />
                            </svg>
                            <span><strong className="font-semibold">Direct Pipe needs a base video. </strong>Select one above before queueing this render.</span>
                        </div>
                    )}

                    {dispatchError && (
                        <div className="flex items-start gap-2 px-3 py-2.5 bg-red-950/40 border border-red-800/50 rounded-lg text-xs text-red-400">
                            <svg className="shrink-0 mt-0.5" width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
                                <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" />
                            </svg>
                            <span><strong className="font-semibold">Dispatch failed: </strong>{dispatchError}</span>
                        </div>
                    )}
                    <button type="button" onClick={handleDispatch} disabled={!canDispatch}
                            className={`w-full py-3 text-sm font-bold rounded-xl transition-all tracking-wide ${
                                canDispatch
                                    ? "bg-violet-600 hover:bg-violet-500 text-white shadow-lg shadow-violet-900/40 active:scale-[0.99]"
                                    : "bg-neutral-800 text-neutral-600 cursor-not-allowed"
                            }`}>
                        {isDispatching
                            ? <span className="flex items-center justify-center gap-2">
                                <span className="w-3.5 h-3.5 border-2 border-violet-400/40 border-t-white/80 rounded-full animate-spin" />
                                Queuing…
                              </span>
                            : "Queue Render"}
                    </button>
                </>
            )}

            {/* ── BATCH ────────────────────────────────────────────────────────── */}
            {mode === "batch" && (
                <>
                    {/* ═══════════════════════════════════════════════════════════════
                        ZONE 1 — SHARED TEMPLATE + FAST IMPORT
                    ═══════════════════════════════════════════════════════════════ */}
                    <div className="rounded-2xl border border-violet-700/50 overflow-hidden shadow-xl shadow-violet-950/10 bg-neutral-950/50">
                        <div className="flex items-center gap-3 px-4 py-3 bg-linear-to-r from-violet-950/60 via-violet-950/30 to-neutral-950 border-b border-violet-800/40">
                            <div className="flex items-center justify-center w-8 h-8 rounded-lg bg-violet-600/20 border border-violet-500/30 text-violet-300 shrink-0">
                                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
                                    <rect x="2" y="2" width="12" height="12" rx="2" />
                                    <path d="M5 2v12M11 2v12M2 6h12M2 10h12" opacity=".45" />
                                </svg>
                            </div>
                            <div className="min-w-0 flex-1">
                                <div className="flex items-center gap-2">
                                    <span className="text-[11px] font-bold uppercase tracking-[0.12em] text-violet-200">Batch template</span>
                                    <span className="text-[9px] px-1.5 py-0.5 rounded-full bg-violet-600 text-white font-bold">shared</span>
                                </div>
                                <p className="text-[10px] text-violet-300/55 mt-0.5">Geometry, styling, Direct Pipe behavior and every non-source render option live here first.</p>
                            </div>
                            <span className="text-[9px] text-violet-400/60 font-mono shrink-0">{batchItems.length} item{batchItems.length !== 1 ? "s" : ""}</span>
                        </div>

                        <div className="px-4 py-4 space-y-4">
                            {/* Shared output destination */}
                            <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_auto] gap-3 items-end">
                                <div>
                                    <div className="flex items-center gap-2 mb-1">
                                        <FieldLabel>Template output folder</FieldLabel>
                                        <span className="text-[9px] px-1.5 py-0.5 rounded bg-violet-950/40 text-violet-300 border border-violet-800/40">all items inherit</span>
                                    </div>
                                    <BrowseInput value={batchOutputDirectory} onChange={setBatchOutputDirectory}
                                                 onBrowse={async () => {
                                                     try {
                                                         const sel = await open({ multiple: false, directory: true });
                                                         if (typeof sel === "string") setBatchOutputDirectory(sel);
                                                     } catch {}
                                                 }}
                                                 onClear={() => setBatchOutputDirectory("")}
                                                 placeholder="Choose one destination for the whole batch" browseLabel="Select folder" />
                                    {!batchOutputDirectory && (
                                        <p className="text-[9px] text-neutral-700 mt-1.5">Until a folder is selected, imported items fall back to the source folder.</p>
                                    )}
                                </div>
                                <div className="flex items-center justify-end gap-1.5">
                                    <button type="button" onClick={() => setBatchOutputDirectory(getPathDirectory(opts.outputPath || ""))} disabled={!getPathDirectory(opts.outputPath || "")}
                                            className="px-2.5 py-1.5 rounded-md border border-neutral-800 bg-neutral-900 text-[10px] text-neutral-500 hover:text-neutral-300 hover:border-neutral-700 disabled:opacity-30 disabled:cursor-not-allowed">
                                        Use current output folder
                                    </button>
                                </div>
                            </div>

                            {/* Fast JSONL + optional matching video importer */}
                            <div className="rounded-xl border border-neutral-800 bg-neutral-900/55 overflow-hidden">
                                <div className="px-3.5 py-3 border-b border-neutral-800/80 flex items-center gap-2.5">
                                    <div className="w-7 h-7 rounded-md bg-emerald-950/60 border border-emerald-800/50 flex items-center justify-center text-emerald-400">
                                        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                                            <path d="M3 5h10M3 8h6M3 11h8" />
                                            <path d="M12 7v6M9 10l3 3 3-3" />
                                        </svg>
                                    </div>
                                    <div className="flex-1 min-w-0">
                                        <div className="text-[10px] font-bold uppercase tracking-[0.12em] text-neutral-300">Import chat logs + optional cut videos</div>
                                        <div className="text-[9px] text-neutral-600 mt-0.5">Matches identical filename stems. Matching videos are attached automatically; they are only required for items using Direct Pipe mode.</div>
                                    </div>
                                </div>
                                <div className="p-3.5 space-y-3">
                                    <div className="grid grid-cols-1 sm:grid-cols-[auto_auto_minmax(0,1fr)] gap-2 items-end">
                                        <div>
                                            <FieldLabel>Match mode</FieldLabel>
                                            <PillTabs options={[{ value: "glob" as BatchMatchMode, label: "Glob" }, { value: "regex" as BatchMatchMode, label: "Regex" }]} value={batchMatchMode} onChange={setBatchMatchMode} />
                                        </div>
                                        <div className="sm:col-span-2">
                                            <div className="flex items-center mb-1"><FieldLabel>{batchMatchMode === "glob" ? "Filename stem pattern" : "Stem regex"}</FieldLabel><span className="text-[9px] text-neutral-700 ml-2">example: <span className="font-mono">*-cut*</span></span></div>
                                            <TextInput value={batchPattern} onChange={setBatchPattern} placeholder="*-cut*" mono />
                                        </div>
                                    </div>
                                    <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
                                        <button type="button" onClick={scanSelectedBatchFiles} disabled={batchImporting}
                                                className="flex items-center justify-center gap-2 px-3 py-2.5 rounded-lg bg-violet-600 hover:bg-violet-500 text-white text-[10px] font-bold disabled:opacity-40 transition-all">
                                            {batchImporting ? "Scanning…" : "Select files"}
                                        </button>
                                        <button type="button" onClick={scanBatchFolder} disabled={batchImporting}
                                                className="flex items-center justify-center gap-2 px-3 py-2.5 rounded-lg bg-neutral-800 hover:bg-neutral-700 border border-neutral-700 text-neutral-200 text-[10px] font-bold disabled:opacity-40 transition-all">
                                            Select folder
                                        </button>
                                        <button type="button" onClick={addBatchItem}
                                                className="flex items-center justify-center gap-2 px-3 py-2.5 rounded-lg bg-neutral-900 hover:bg-neutral-800 border border-dashed border-neutral-700 text-neutral-500 hover:text-neutral-300 text-[10px] font-semibold transition-all">
                                            + Manual item
                                        </button>
                                    </div>
                                    <div className="flex flex-wrap items-center gap-2 text-[9px] text-neutral-700">
                                        <span className="font-mono">name123-cut01.jsonl</span><span>↔</span><span className="font-mono">name123-cut01.mp4</span>
                                        <span className="ml-auto">JSONL-only imports are valid; matching videos are auto-attached when present.</span>
                                    </div>
                                    {batchImportNotice && (
                                        <div className="px-3 py-2 rounded-lg bg-emerald-950/30 border border-emerald-800/40 text-[10px] text-emerald-400">{batchImportNotice}</div>
                                    )}
                                    {batchImportError && (
                                        <div className="px-3 py-2 rounded-lg bg-red-950/30 border border-red-800/40 text-[10px] text-red-400">{batchImportError}</div>
                                    )}
                                </div>
                            </div>

                            <SettingsPanel
                                opts={opts} setOpt={setOpt} setOpts={setOpts}
                                isBatchItem={false}
                                showTemplateCanvas
                                overlayVideoPath={templatePreviewVideoPath}
                                onSelectOverlayVideo={handleSelectTemplatePreviewVideo}
                                videoMeta={templatePreviewVideoMeta}
                                videoLoading={templatePreviewVideoLoading}
                                videoError={templatePreviewVideoError}
                                onSetOverlayVideoPath={setTemplatePreviewVideoPath}
                                previewOnlyBaseVideo
                            />
                        </div>
                    </div>

                    {/* ═══════════════════════════════════════════════════════════════
                        ZONE 2 — RENDER ITEMS
                    ═══════════════════════════════════════════════════════════════ */}
                    <div className="space-y-2.5">
                        <div className="flex items-center justify-between gap-2">
                            <div className="flex items-center gap-2">
                                <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-neutral-800 border border-neutral-700 text-neutral-300 text-[10px] font-bold tracking-wider uppercase">
                                    <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><rect x="1" y="1" width="6" height="6" rx="1" /></svg>
                                    Render items
                                </span>
                                <span className="text-[10px] text-neutral-600">
                                    {batchItems.length} total{readyBatchItems.length > 0 && <> · <span className="text-emerald-500 font-medium">{readyBatchItems.length} ready</span></>}
                                </span>
                            </div>
                            <span className="text-[9px] text-neutral-700 hidden sm:block">collapsed by default · explicit changes become overrides</span>
                        </div>

                        <div className="space-y-2">
                            {batchItems.map((item, index) => (
                                <BatchItemPanel
                                    key={item.id}
                                    item={item}
                                    index={index}
                                    mainOpts={opts}
                                    allItems={batchItems}
                                    sharedOutputDirectory={batchOutputDirectory}
                                    onChange={(updated) => updateBatchItem(item.id, updated)}
                                    onRemove={() => removeBatchItem(item.id)}
                                    onCopyFrom={(si) => copySettingsFromItem(item.id, si)}
                                />
                            ))}

                            <button type="button" onClick={addBatchItem}
                                    className="w-full py-3 border border-dashed border-neutral-800 hover:border-violet-600/50 rounded-xl text-xs text-neutral-600 hover:text-violet-400 transition-all flex items-center justify-center gap-2 group">
                                <span className="w-5 h-5 rounded-full border border-dashed border-current flex items-center justify-center"><svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"><path d="M4 1v6M1 4h6" /></svg></span>
                                Add render item
                                <span className="text-[9px] text-neutral-700 group-hover:text-violet-600/60 transition-colors">— inherits the template</span>
                            </button>
                        </div>
                    </div>

                    {/* ── Dispatch footer ───────────────────────────────────────── */}
                    <div className="space-y-2">
                        {batchDispatchError && (
                            <div className="flex items-start gap-2 px-3 py-2.5 bg-red-950/40 border border-red-800/50 rounded-lg text-xs text-red-400">
                                <svg className="shrink-0 mt-0.5" width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" /></svg>
                                <span><strong className="font-semibold">Batch dispatch failed: </strong>{batchDispatchError}</span>
                            </div>
                        )}

                        {batchItems.length > 0 && (
                            <div className="flex flex-wrap items-center gap-2 px-3 py-2.5 bg-neutral-900/60 border border-neutral-800 rounded-lg text-[10px]">
                                <span className={`flex items-center gap-1.5 font-medium ${readyBatchItems.length > 0 ? "text-emerald-400" : "text-neutral-600"}`}>
                                    <span className={`w-1.5 h-1.5 rounded-full ${readyBatchItems.length > 0 ? "bg-emerald-400" : "bg-neutral-700"}`} />
                                    {readyBatchItems.length} ready
                                </span>
                                {batchItems.length - readyBatchItems.length > 0 && (
                                    <span className="flex items-center gap-1.5 text-neutral-600"><span className="w-1.5 h-1.5 rounded-full bg-neutral-700" />{batchItems.length - readyBatchItems.length} incomplete</span>
                                )}
                                <span className="ml-auto text-neutral-700">Template geometry shared · base videos item-specific</span>
                            </div>
                        )}

                        <button type="button" onClick={handleBatchDispatch} disabled={readyBatchItems.length === 0 || batchDispatching}
                                className={`w-full py-3 text-sm font-bold rounded-xl transition-all tracking-wide ${
                                    readyBatchItems.length > 0 && !batchDispatching
                                        ? "bg-violet-600 hover:bg-violet-500 text-white shadow-lg shadow-violet-900/40 active:scale-[0.99]"
                                        : "bg-neutral-800 text-neutral-600 cursor-not-allowed"
                                }`}>
                            {batchDispatching ? (
                                <span className="flex items-center justify-center gap-2"><span className="w-3.5 h-3.5 border-2 border-violet-400/40 border-t-white/80 rounded-full animate-spin" />Sending {readyBatchItems.length} render{readyBatchItems.length !== 1 ? "s" : ""} to backend…</span>
                            ) : readyBatchItems.length === 0 ? (
                                "Add a JSONL item to begin (and a base video for Direct Pipe items)"
                            ) : (
                                <span className="flex items-center justify-center gap-2"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M3 8h10M8 3l5 5-5 5" /></svg>Queue {readyBatchItems.length} render{readyBatchItems.length !== 1 ? "s" : ""}</span>
                            )}
                        </button>
                    </div>
                </>
            )}
        </div>
    );
}