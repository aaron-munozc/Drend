/**
 * RenderForm.tsx
 * Fully redesigned render configuration form with:
 *  - Interactive visual canvas editor (drag + resize overlays)
 *  - Shape + image overlays draggable/resizable directly on the preview
 *  - Frame extraction delegated to Rust backend (invoke "extract_video_frame")
 *  - Collapsible sections to keep the panel compact
 *  - Zustand-backed state (imported via useWorkspace shim)
 */

import { useNavigate } from "@tanstack/react-router";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import React, {
	useCallback,
	useState,
	useRef,
	useEffect,
	useMemo,
} from "react";
import {TabState, useWorkspace} from "@/stores/useWorkspaceStore.ts";
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
	frameDataUrl: string; // base64 jpeg from Rust or canvas fallback
}

type OverlayKind = "chat" | "shape" | "image";

interface DragTarget {
	kind: OverlayKind;
	index?: number; // for shape / image
	edge?: "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw" | null;
	startX: number;
	startY: number;
	initRect: { x: number; y: number; width: number; height: number };
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

const EVICTION_MODES: {
	value: EvictionStrategy;
	label: string;
	description: string;
}[] = [
	{
		value: "pushOnly",
		label: "Push Only",
		description: "Messages stay until pushed off-screen",
	},
	{
		value: "timed",
		label: "Timed",
		description: "Messages fade out after a set duration",
	},
];

const QUALITY_PRESETS: {
	value: QualityPreset;
	label: string;
	description: string;
}[] = [
	{ value: "draft", label: "Draft", description: "Fastest · nearest-neighbor" },
	{ value: "standard", label: "Standard", description: "Bilinear · balanced" },
	{ value: "high", label: "High", description: "Lanczos3 · best quality" },
];

const TIMELINE_STRATEGIES: {
	value: TimelineMismatchStrategy;
	label: string;
	description: string;
}[] = [
	{
		value: "freezeLastFrame",
		label: "Freeze",
		description: "Hold last chat frame over remaining video",
	},
	{
		value: "renderClearCanvas",
		label: "Clear",
		description: "Pass remaining video frames through clean",
	},
	{
		value: "loopChatLog",
		label: "Loop",
		description: "Restart the chat overlay from the beginning",
	},
];

const RESIZE_HANDLE_SIZE = 8; // px in screen space

// ─────────────────────────────────────────────────────────────────────────────
// Primitive UI
// ─────────────────────────────────────────────────────────────────────────────

function SectionHeader({
						   children,
						   open,
						   onToggle,
					   }: {
	children: React.ReactNode;
	open: boolean;
	onToggle: () => void;
}) {
	return (
		<button
			type="button"
			onClick={onToggle}
			className="flex items-center justify-between w-full group"
		>
			<h4 className="text-[10px] font-bold text-neutral-500 uppercase tracking-[0.15em]">
				{children}
			</h4>
			<svg
				width="10"
				height="10"
				viewBox="0 0 10 10"
				fill="none"
				stroke="currentColor"
				strokeWidth="1.8"
				strokeLinecap="round"
				className={`text-neutral-600 transition-transform duration-200 ${open ? "rotate-180" : ""}`}
			>
				<path d="M2 3.5l3 3 3-3" />
			</svg>
		</button>
	);
}

function Divider() {
	return <div className="border-b border-neutral-800/80 my-0.5" />;
}

function FieldLabel({ children }: { children: React.ReactNode }) {
	return (
		<label className="block text-[10px] font-semibold text-neutral-500 mb-1 uppercase tracking-wide">
			{children}
		</label>
	);
}

function TextInput({
					   value,
					   onChange,
					   onBlur,
					   placeholder,
					   mono,
					   disabled,
				   }: {
	value: string;
	onChange: (v: string) => void;
	onBlur?: () => void;
	placeholder?: string;
	mono?: boolean;
	disabled?: boolean;
}) {
	return (
		<input
			type="text"
			value={value}
			placeholder={placeholder}
			disabled={disabled}
			onChange={(e) => onChange(e.target.value)}
			onBlur={onBlur}
			className={`w-full bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all disabled:opacity-40 disabled:cursor-not-allowed ${mono ? "font-mono" : ""}`}
		/>
	);
}

function NumberInput({
						 value,
						 onChange,
						 min,
						 max,
						 step,
					 }: {
	value: number;
	onChange: (v: number) => void;
	min?: number;
	max?: number;
	step?: number;
}) {
	return (
		<input
			type="number"
			value={value}
			min={min}
			max={max}
			step={step}
			onChange={(e) => onChange(Number(e.target.value))}
			className="w-full bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all"
		/>
	);
}

function Toggle({
					value,
					onChange,
					label,
					description,
				}: {
	value: boolean;
	onChange: (v: boolean) => void;
	label: string;
	description?: string;
}) {
	return (
		<button
			type="button"
			onClick={() => onChange(!value)}
			className="flex items-start gap-2.5 w-full text-left group"
		>
			<div
				className={`mt-0.5 relative w-7 h-4 rounded-full flex-shrink-0 transition-colors duration-200 ${value ? "bg-indigo-600" : "bg-neutral-700"}`}
			>
				<div
					className={`absolute top-0.5 w-3 h-3 rounded-full bg-white shadow-sm transition-transform duration-200 ${value ? "translate-x-3.5" : "translate-x-0.5"}`}
				/>
			</div>
			<div>
				<div className="text-xs text-neutral-300 font-medium leading-tight">
					{label}
				</div>
				{description && (
					<div className="text-[10px] text-neutral-600 mt-0.5">{description}</div>
				)}
			</div>
		</button>
	);
}

function BrowseInput({
						 value,
						 onChange,
						 onBrowse,
						 placeholder,
						 browseLabel,
					 }: {
	value: string;
	onChange: (v: string) => void;
	onBrowse: () => void;
	placeholder: string;
	browseLabel: string;
}) {
	return (
		<div className="flex gap-1.5">
			<input
				type="text"
				placeholder={placeholder}
				value={value}
				onChange={(e) => onChange(e.target.value)}
				className="flex-1 bg-neutral-900 border border-neutral-700/80 rounded-md px-2.5 py-1.5 text-xs text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500/70 focus:ring-1 focus:ring-indigo-500/20 transition-all font-mono"
			/>
			<button
				type="button"
				onClick={onBrowse}
				className="px-2.5 py-1.5 bg-neutral-800 border border-neutral-700/80 hover:bg-neutral-750 hover:border-neutral-600 text-neutral-300 text-xs font-medium rounded-md transition-all whitespace-nowrap"
			>
				{browseLabel}
			</button>
		</div>
	);
}

function SegmentedControl<T extends string>({
												options,
												value,
												onChange,
											}: {
	options: { value: T; label: string; description?: string }[];
	value: T;
	onChange: (v: T) => void;
}) {
	return (
		<div className="flex flex-col gap-0.5">
			{options.map((o) => (
				<button
					key={o.value}
					type="button"
					onClick={() => onChange(o.value)}
					className={`flex items-center gap-2.5 px-2.5 py-1.5 rounded-md text-left transition-all ${
						value === o.value
							? "bg-indigo-600/20 border border-indigo-500/40 text-indigo-300"
							: "border border-transparent text-neutral-500 hover:bg-neutral-800/60 hover:text-neutral-300"
					}`}
				>
					<div
						className={`w-2.5 h-2.5 rounded-full border-2 flex-shrink-0 transition-colors ${
							value === o.value
								? "border-indigo-400 bg-indigo-400"
								: "border-neutral-600"
						}`}
					/>
					<div>
						<span className="text-xs font-medium">{o.label}</span>
						{o.description && (
							<span className="text-[10px] text-neutral-600 ml-1.5">
								{o.description}
							</span>
						)}
					</div>
				</button>
			))}
		</div>
	);
}

function PillTabs<T extends string>({
										options,
										value,
										onChange,
									}: {
	options: { value: T; label: string }[];
	value: T;
	onChange: (v: T) => void;
}) {
	return (
		<div className="flex flex-wrap gap-1">
			{options.map((o) => (
				<button
					key={o.value}
					type="button"
					onClick={() => onChange(o.value)}
					className={`px-2.5 py-1 text-[11px] rounded-md font-medium transition-all ${
						value === o.value
							? "bg-indigo-600 text-white shadow-sm shadow-indigo-900/50"
							: "bg-neutral-800 text-neutral-500 hover:text-neutral-300 border border-neutral-700/60 hover:border-neutral-600"
					}`}
				>
					{o.label}
				</button>
			))}
		</div>
	);
}

// Collapsible section wrapper
function Section({
					 title,
					 defaultOpen = true,
					 children,
				 }: {
	title: string;
	defaultOpen?: boolean;
	children: React.ReactNode;
}) {
	const [open, setOpen] = useState(defaultOpen);
	return (
		<div className="space-y-0">
			<SectionHeader open={open} onToggle={() => setOpen((v) => !v)}>
				{title}
			</SectionHeader>
			<Divider />
			{open && <div className="pt-3 space-y-3">{children}</div>}
		</div>
	);
}

// ─────────────────────────────────────────────────────────────────────────────
// Interactive Canvas Editor
// ─────────────────────────────────────────────────────────────────────────────

interface CanvasEditorProps {
	videoMeta: VideoMeta;
	opts: RenderVideoArgs;
	setOpt: <K extends keyof RenderVideoArgs>(
		key: K,
		value: RenderVideoArgs[K],
	) => void;
}

function ResizeHandle({
						  edge,
						  onPointerDown,
					  }: {
	edge: string;
	onPointerDown: (e: React.PointerEvent) => void;
}) {
	const edgeStyles: Record<string, React.CSSProperties> = {
		n: { top: -4, left: "50%", transform: "translateX(-50%)", cursor: "n-resize" },
		s: { bottom: -4, left: "50%", transform: "translateX(-50%)", cursor: "s-resize" },
		e: { right: -4, top: "50%", transform: "translateY(-50%)", cursor: "e-resize" },
		w: { left: -4, top: "50%", transform: "translateY(-50%)", cursor: "w-resize" },
		ne: { top: -4, right: -4, cursor: "ne-resize" },
		nw: { top: -4, left: -4, cursor: "nw-resize" },
		se: { bottom: -4, right: -4, cursor: "se-resize" },
		sw: { bottom: -4, left: -4, cursor: "sw-resize" },
	};
	return (
		<div
			style={{
				position: "absolute",
				width: RESIZE_HANDLE_SIZE,
				height: RESIZE_HANDLE_SIZE,
				borderRadius: 2,
				background: "white",
				border: "1.5px solid rgba(99,102,241,0.9)",
				zIndex: 30,
				...edgeStyles[edge],
			}}
			onPointerDown={(e) => {
				e.stopPropagation();
				onPointerDown(e);
			}}
		/>
	);
}

function CanvasEditor({ videoMeta, opts, setOpt }: CanvasEditorProps) {
	const containerRef = useRef<HTMLDivElement>(null);
	const dragRef = useRef<DragTarget | null>(null);
	const [selected, setSelected] = useState<{
		kind: OverlayKind;
		index?: number;
	} | null>({ kind: "chat" });

	const toVideo = useCallback(
		(screenPx: number, dimension: "w" | "h") => {
			if (!containerRef.current) return 0;
			const rect = containerRef.current.getBoundingClientRect();
			const dim = dimension === "w" ? videoMeta.width : videoMeta.height;
			const screenDim = dimension === "w" ? rect.width : rect.height;
			return (screenPx / screenDim) * dim;
		},
		[videoMeta],
	);

	const startDrag = useCallback(
		(
			e: React.PointerEvent,
			kind: OverlayKind,
			index: number | undefined,
			initRect: DragTarget["initRect"],
			edge: DragTarget["edge"] = null,
		) => {
			e.preventDefault();
			e.stopPropagation();
			(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
			dragRef.current = {
				kind,
				index,
				edge,
				startX: e.clientX,
				startY: e.clientY,
				initRect,
			};
			setSelected({ kind, index });
		},
		[],
	);

	const onPointerMove = useCallback(
		(e: React.PointerEvent) => {
			const d = dragRef.current;
			if (!d) return;
			const dx = toVideo(e.clientX - d.startX, "w");
			const dy = toVideo(e.clientY - d.startY, "h");

			const applyMove = (
				rect: DragTarget["initRect"],
			): DragTarget["initRect"] => {
				if (!d.edge) {
					// Pure move
					return {
						...rect,
						x: Math.round(rect.x + dx),
						y: Math.round(rect.y + dy),
					};
				}
				// Resize
				let { x, y, width, height } = rect;
				if (d.edge.includes("e")) width = Math.max(20, Math.round(rect.width + dx));
				if (d.edge.includes("s")) height = Math.max(20, Math.round(rect.height + dy));
				if (d.edge.includes("w")) {
					const newW = Math.max(20, Math.round(rect.width - dx));
					x = Math.round(rect.x + (rect.width - newW));
					width = newW;
				}
				if (d.edge.includes("n")) {
					const newH = Math.max(20, Math.round(rect.height - dy));
					y = Math.round(rect.y + (rect.height - newH));
					height = newH;
				}
				return { x, y, width, height };
			};

			if (d.kind === "chat") {
				const r = applyMove(d.initRect);
				setOpt("overlayX", r.x);
				setOpt("overlayY", r.y);
				setOpt("overlayWidth", r.width);
				setOpt("overlayHeight", r.height);
			} else if (d.kind === "shape" && d.index !== undefined) {
				const r = applyMove(d.initRect);
				const next = [...opts.shapeOverlays];
				next[d.index] = { ...next[d.index], x: r.x, y: r.y, width: r.width, height: r.height };
				setOpt("shapeOverlays", next);
			} else if (d.kind === "image" && d.index !== undefined) {
				const r = applyMove(d.initRect);
				const next = [...opts.imageOverlays];
				next[d.index] = {
					...next[d.index],
					x: r.x,
					y: r.y,
					width: r.width || undefined,
					height: r.height || undefined,
				};
				setOpt("imageOverlays", next);
			}
		},
		[opts, setOpt, toVideo],
	);

	const onPointerUp = useCallback((e: React.PointerEvent) => {
		if (dragRef.current) {
			const target = e.currentTarget as HTMLElement;
			if (target.hasPointerCapture(e.pointerId)) {
				target.releasePointerCapture(e.pointerId);
			}
			dragRef.current = null;
		}
	}, []);

	const pct = (v: number, dim: number) => `${(v / dim) * 100}%`;

	const chatW = opts.overlayWidth ?? opts.width;
	const chatH = opts.overlayHeight ?? opts.height;
	const chatX = opts.overlayX ?? 0;
	const chatY = opts.overlayY ?? 0;

	return (
		<div className="space-y-2">
			{/* Canvas info bar */}
			<div className="flex items-center justify-between text-[10px] font-medium">
				<span className="text-neutral-400">Visual Editor</span>
				<span className="text-neutral-600 font-mono">
					{videoMeta.width} × {videoMeta.height} · {Math.round(videoMeta.duration)}s
				</span>
			</div>

			{/* Legend */}
			<div className="flex flex-wrap gap-3 text-[10px] text-neutral-500">
				<span className="flex items-center gap-1">
					<span className="w-2.5 h-2.5 rounded-sm border border-violet-500 bg-violet-600/20 inline-block" />
					Chat overlay
				</span>
				{opts.shapeOverlays.length > 0 && (
					<span className="flex items-center gap-1">
						<span className="w-2.5 h-2.5 rounded-sm border border-indigo-400 border-dashed inline-block" />
						Shapes
					</span>
				)}
				{opts.imageOverlays.length > 0 && (
					<span className="flex items-center gap-1">
						<span className="w-2.5 h-2.5 rounded-sm border border-emerald-400 border-dashed inline-block" />
						Images
					</span>
				)}
			</div>

			{/* The canvas */}
			<div
				ref={containerRef}
				className="relative w-full rounded-lg overflow-hidden border border-neutral-700 bg-neutral-950 cursor-default"
				style={{ aspectRatio: `${videoMeta.width} / ${videoMeta.height}` }}
				onPointerMove={onPointerMove}
				onPointerUp={onPointerUp}
				onPointerCancel={onPointerUp}
				onClick={() => setSelected(null)}
			>
				{/* Background frame */}
				<img
					src={videoMeta.frameDataUrl}
					className="absolute inset-0 w-full h-full object-cover pointer-events-none"
					style={{ opacity: 0.55 }}
					alt="Video reference frame"
					draggable={false}
				/>

				{/* Checkerboard pattern (shows alpha regions) */}
				<div
					className="absolute inset-0 pointer-events-none"
					style={{
						backgroundImage:
							"repeating-conic-gradient(#111 0% 25%, transparent 0% 50%)",
						backgroundSize: "16px 16px",
						opacity: 0.12,
					}}
				/>

				{/* Shape overlays */}
				{opts.shapeOverlays.map((s, i) => {
					const isSel = selected?.kind === "shape" && selected.index === i;
					return (
						<div
							key={`shape-${i}`}
							className={`absolute transition-shadow ${isSel ? "ring-1 ring-indigo-400" : ""}`}
							style={{
								left: pct(s.x, videoMeta.width),
								top: pct(s.y, videoMeta.height),
								width: pct(s.width, videoMeta.width),
								height: pct(s.height, videoMeta.height),
								backgroundColor: `rgba(${s.color.red},${s.color.green},${s.color.blue},${s.color.alpha / 255})`,
								borderRadius: `${(s.cornerRadius / Math.min(s.width, s.height)) * 50}%`,
								border: isSel
									? "1.5px solid rgba(99,102,241,0.8)"
									: "1.5px dashed rgba(99,102,241,0.4)",
								cursor: "move",
								zIndex: 10,
							}}
							onPointerDown={(e) =>
								startDrag(e, "shape", i, {
									x: s.x,
									y: s.y,
									width: s.width,
									height: s.height,
								})
							}
						>
							<span className="absolute top-1 left-1 text-[8px] font-bold text-indigo-300/70 pointer-events-none select-none leading-none">
								Shape {i + 1}
							</span>
							{isSel &&
								(["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const).map(
									(edge) => (
										<ResizeHandle
											key={edge}
											edge={edge}
											onPointerDown={(e) =>
												startDrag(
													e,
													"shape",
													i,
													{ x: s.x, y: s.y, width: s.width, height: s.height },
													edge,
												)
											}
										/>
									),
								)}
						</div>
					);
				})}

				{/* Image overlays */}
				{opts.imageOverlays.map((ov, i) => {
					const isSel = selected?.kind === "image" && selected.index === i;
					const w = ov.width ?? 120;
					const h = ov.height ?? 80;
					return (
						<div
							key={`image-${i}`}
							className={`absolute ${isSel ? "ring-1 ring-emerald-400" : ""}`}
							style={{
								left: pct(ov.x, videoMeta.width),
								top: pct(ov.y, videoMeta.height),
								width: pct(w, videoMeta.width),
								height: pct(h, videoMeta.height),
								opacity: ov.alpha,
								border: isSel
									? "1.5px solid rgba(52,211,153,0.8)"
									: "1.5px dashed rgba(52,211,153,0.4)",
								cursor: "move",
								zIndex: 11,
								backgroundImage: ov.assetPath
									? `url(${convertFileSrc(ov.assetPath)})`
									: undefined,
								backgroundSize: "cover",
								backgroundPosition: "center",
								backgroundColor: "rgba(52,211,153,0.08)",
							}}
							onPointerDown={(e) =>
								startDrag(e, "image", i, { x: ov.x, y: ov.y, width: w, height: h })
							}
						>
							{!ov.assetPath && (
								<span className="absolute inset-0 flex items-center justify-center text-[8px] font-bold text-emerald-400/60 pointer-events-none select-none">
									IMG {i + 1}
								</span>
							)}
							{isSel &&
								(["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const).map(
									(edge) => (
										<ResizeHandle
											key={edge}
											edge={edge}
											onPointerDown={(e) =>
												startDrag(
													e,
													"image",
													i,
													{ x: ov.x, y: ov.y, width: w, height: h },
													edge,
												)
											}
										/>
									),
								)}
						</div>
					);
				})}

				{/* Chat overlay — always topmost draggable */}
				{(() => {
					const isSel = selected?.kind === "chat";
					return (
						<div
							className={`absolute backdrop-blur-[1px] flex items-center justify-center ${isSel ? "ring-1 ring-violet-500" : ""}`}
							style={{
								left: pct(chatX, videoMeta.width),
								top: pct(chatY, videoMeta.height),
								width: pct(chatW, videoMeta.width),
								height: pct(chatH, videoMeta.height),
								backgroundColor: "rgba(109,40,217,0.20)",
								border: isSel
									? "2px solid rgba(139,92,246,0.9)"
									: "2px dashed rgba(139,92,246,0.5)",
								cursor: "move",
								zIndex: 20,
							}}
							onPointerDown={(e) =>
								startDrag(e, "chat", undefined, {
									x: chatX,
									y: chatY,
									width: chatW,
									height: chatH,
								})
							}
						>
							<div className="flex flex-col items-center pointer-events-none select-none">
								<span className="text-violet-200 text-[9px] font-bold px-1.5 py-0.5 bg-violet-900/70 rounded-sm leading-none">
									CHAT
								</span>
								{isSel && (
									<span className="text-violet-300/60 text-[8px] mt-0.5 font-mono">
										{chatW}×{chatH}
									</span>
								)}
							</div>
							{isSel &&
								(["n", "s", "e", "w", "ne", "nw", "se", "sw"] as const).map(
									(edge) => (
										<ResizeHandle
											key={edge}
											edge={edge}
											onPointerDown={(e) =>
												startDrag(
													e,
													"chat",
													undefined,
													{ x: chatX, y: chatY, width: chatW, height: chatH },
													edge,
												)
											}
										/>
									),
								)}
						</div>
					);
				})()}

				{/* Deselect hint */}
				{selected && (
					<div
						className="absolute bottom-1.5 right-1.5 text-[9px] text-neutral-500 bg-neutral-900/70 px-1.5 py-0.5 rounded pointer-events-none select-none"
					>
						Click canvas to deselect
					</div>
				)}
			</div>

			{/* Numeric readout for selected item */}
			{selected && (
				<div className="grid grid-cols-4 gap-1.5 pt-1">
					{selected.kind === "chat" && (
						<>
							<div>
								<FieldLabel>X</FieldLabel>
								<NumberInput
									value={chatX}
									onChange={(v) => setOpt("overlayX", v)}
								/>
							</div>
							<div>
								<FieldLabel>Y</FieldLabel>
								<NumberInput
									value={chatY}
									onChange={(v) => setOpt("overlayY", v)}
								/>
							</div>
							<div>
								<FieldLabel>W</FieldLabel>
								<NumberInput
									value={chatW}
									min={1}
									onChange={(v) => setOpt("overlayWidth", v)}
								/>
							</div>
							<div>
								<FieldLabel>H</FieldLabel>
								<NumberInput
									value={chatH}
									min={1}
									onChange={(v) => setOpt("overlayHeight", v)}
								/>
							</div>
						</>
					)}
					{selected.kind === "shape" && selected.index !== undefined && (() => {
						const s = opts.shapeOverlays[selected.index];
						if (!s) return null;
						const upd = (patch: Partial<CustomShapeOverlay>) => {
							const next = [...opts.shapeOverlays];
							next[selected.index!] = { ...s, ...patch };
							setOpt("shapeOverlays", next);
						};
						return (
							<>
								<div><FieldLabel>X</FieldLabel><NumberInput value={s.x} onChange={(v) => upd({ x: v })} /></div>
								<div><FieldLabel>Y</FieldLabel><NumberInput value={s.y} onChange={(v) => upd({ y: v })} /></div>
								<div><FieldLabel>W</FieldLabel><NumberInput value={s.width} min={1} onChange={(v) => upd({ width: v })} /></div>
								<div><FieldLabel>H</FieldLabel><NumberInput value={s.height} min={1} onChange={(v) => upd({ height: v })} /></div>
							</>
						);
					})()}
					{selected.kind === "image" && selected.index !== undefined && (() => {
						const ov = opts.imageOverlays[selected.index];
						if (!ov) return null;
						const upd = (patch: Partial<CustomImageOverlay>) => {
							const next = [...opts.imageOverlays];
							next[selected.index!] = { ...ov, ...patch };
							setOpt("imageOverlays", next);
						};
						return (
							<>
								<div><FieldLabel>X</FieldLabel><NumberInput value={ov.x} onChange={(v) => upd({ x: v })} /></div>
								<div><FieldLabel>Y</FieldLabel><NumberInput value={ov.y} onChange={(v) => upd({ y: v })} /></div>
								<div><FieldLabel>W</FieldLabel><NumberInput value={ov.width ?? 0} min={0} onChange={(v) => upd({ width: v || undefined })} /></div>
								<div><FieldLabel>H</FieldLabel><NumberInput value={ov.height ?? 0} min={0} onChange={(v) => upd({ height: v || undefined })} /></div>
							</>
						);
					})()}
				</div>
			)}
		</div>
	);
}

// ─────────────────────────────────────────────────────────────────────────────
// Overlay list editors (below the canvas)
// ─────────────────────────────────────────────────────────────────────────────

const DEFAULT_SHAPE: CustomShapeOverlay = {
	x: 20,
	y: 20,
	width: 200,
	height: 100,
	color: { alpha: 200, red: 30, green: 30, blue: 30 },
	cornerRadius: 8,
};
const DEFAULT_IMAGE_OVERLAY: CustomImageOverlay = {
	assetPath: "",
	x: 0,
	y: 0,
	width: 200,
	height: 150,
	alpha: 1.0,
};

function ShapeOverlayList({
							  overlays,
							  onChange,
						  }: {
	overlays: CustomShapeOverlay[];
	onChange: (v: CustomShapeOverlay[]) => void;
}) {
	const add = () => onChange([...overlays, { ...DEFAULT_SHAPE }]);
	const remove = (i: number) => onChange(overlays.filter((_, idx) => idx !== i));
	const update = (i: number, patch: Partial<CustomShapeOverlay>) => {
		const next = [...overlays];
		next[i] = { ...next[i], ...patch };
		onChange(next);
	};

	return (
		<div className="space-y-1.5">
			{overlays.map((s, i) => (
				<div
					key={i}
					className="bg-neutral-900/80 border border-neutral-700/50 rounded-lg px-3 py-2.5 space-y-2.5"
				>
					<div className="flex items-center justify-between">
						<span className="text-[10px] font-semibold text-indigo-400 uppercase tracking-wide">
							Shape {i + 1}
						</span>
						<button
							type="button"
							onClick={() => remove(i)}
							className="text-neutral-600 hover:text-red-400 text-[10px] transition-colors"
						>
							Remove
						</button>
					</div>
					<div className="grid grid-cols-2 gap-2">
						<div>
							<FieldLabel>Corner Radius</FieldLabel>
							<NumberInput
								value={s.cornerRadius}
								min={0}
								onChange={(v) => update(i, { cornerRadius: v })}
							/>
						</div>
						<ColorPicker
							label="Color"
							value={s.color}
							onChange={(c) => update(i, { color: c })}
						/>
					</div>
				</div>
			))}
			<button
				type="button"
				onClick={add}
				className="w-full py-2 border border-dashed border-neutral-700 hover:border-indigo-500/50 rounded-lg text-[10px] text-neutral-600 hover:text-indigo-400 transition-all"
			>
				+ Add shape overlay
			</button>
		</div>
	);
}

function ImageOverlayList({
							  overlays,
							  onChange,
							  onBrowse,
						  }: {
	overlays: CustomImageOverlay[];
	onChange: (v: CustomImageOverlay[]) => void;
	onBrowse: (index: number) => void;
}) {
	const add = () => onChange([...overlays, { ...DEFAULT_IMAGE_OVERLAY }]);
	const remove = (i: number) => onChange(overlays.filter((_, idx) => idx !== i));
	const update = (i: number, patch: Partial<CustomImageOverlay>) => {
		const next = [...overlays];
		next[i] = { ...next[i], ...patch };
		onChange(next);
	};

	return (
		<div className="space-y-1.5">
			{overlays.map((ov, i) => (
				<div
					key={i}
					className="bg-neutral-900/80 border border-neutral-700/50 rounded-lg px-3 py-2.5 space-y-2.5"
				>
					<div className="flex items-center justify-between">
						<span className="text-[10px] font-semibold text-emerald-500 uppercase tracking-wide">
							Image {i + 1}
						</span>
						<button
							type="button"
							onClick={() => remove(i)}
							className="text-neutral-600 hover:text-red-400 text-[10px] transition-colors"
						>
							Remove
						</button>
					</div>
					<BrowseInput
						value={ov.assetPath}
						onChange={(v) => update(i, { assetPath: v })}
						onBrowse={() => onBrowse(i)}
						placeholder="/path/to/image.png"
						browseLabel="Browse"
					/>
					<CustomSlider
						label="Opacity"
						value={ov.alpha}
						min={0}
						max={1}
						step={0.01}
						unit=""
						onChange={(v) => update(i, { alpha: v })}
					/>
				</div>
			))}
			<button
				type="button"
				onClick={add}
				className="w-full py-2 border border-dashed border-neutral-700 hover:border-emerald-500/50 rounded-lg text-[10px] text-neutral-600 hover:text-emerald-500 transition-all"
			>
				+ Add image overlay
			</button>
		</div>
	);
}

// ─────────────────────────────────────────────────────────────────────────────
// Main form
// ─────────────────────────────────────────────────────────────────────────────

export function RenderForm({ tab, onUpdate }: RenderFormProps) {
	const { registerTaskSnapshot } = useWorkspace();
	const navigate = useNavigate();

	const opts: RenderVideoArgs = useMemo(
		() => ({ ...RENDER_DEFAULTS, ...(tab.renderOptions ?? {}) }),
		[tab.renderOptions],
	);

	const [isDispatching, setIsDispatching] = useState(false);
	const [dispatchError, setDispatchError] = useState<string | null>(null);
	const [videoMeta, setVideoMeta] = useState<VideoMeta | null>(null);
	const [frameLoading, setFrameLoading] = useState(false);

	const [pinnedRaw, setPinnedRaw] = useState(() => opts.pinnedUsers.join(", "));
	const [skipRaw, setSkipRaw] = useState(() => opts.skipUsers.join(", "));

	const setOpt = useCallback(
		<K extends keyof RenderVideoArgs>(key: K, value: RenderVideoArgs[K]) => {
			onUpdate({ renderOptions: { ...opts, [key]: value } });
		},
		[onUpdate, opts],
	);

	const flushUserList = useCallback(
		(key: "pinnedUsers" | "skipUsers", raw: string) => {
			const users = raw
				.split(",")
				.map((s) => s.trim())
				.filter(Boolean);
			setOpt(key, users);
		},
		[setOpt],
	);

	// ── Frame extraction via Rust backend ─────────────────────────────────────
	// The backend command `extract_video_frame` returns a base64 JPEG string.
	// Falls back to the old canvas approach if the command fails.
	const extractFrame = useCallback(async (path: string) => {
		setFrameLoading(true);
		try {
			// Try Rust backend first
			const result = await invoke<{
				width: number;
				height: number;
				duration: number;
				frameBase64: string;
			}>("extract_video_frame", { path, seekSecs: 1.0 });

			setVideoMeta({
				width: result.width,
				height: result.height,
				duration: result.duration,
				frameDataUrl: `data:image/jpeg;base64,${result.frameBase64}`,
			});
		} catch {
			// Fallback: extract in the renderer using a hidden <video> element
			try {
				const video = document.createElement("video");
				video.crossOrigin = "anonymous";
				video.src = convertFileSrc(path);
				video.muted = true;

				await new Promise<void>((resolve, reject) => {
					video.onloadedmetadata = () => {
						video.currentTime = Math.min(1.0, video.duration / 2);
					};
					video.onseeked = () => {
						const canvas = document.createElement("canvas");
						canvas.width = video.videoWidth;
						canvas.height = video.videoHeight;
						const ctx = canvas.getContext("2d");
						if (ctx) {
							ctx.drawImage(video, 0, 0);
							setVideoMeta({
								width: video.videoWidth,
								height: video.videoHeight,
								duration: video.duration,
								frameDataUrl: canvas.toDataURL("image/jpeg", 0.7),
							});
						}
						video.remove();
						resolve();
					};
					video.onerror = reject;
				});
			} catch (fallbackErr) {
				console.warn("Frame extraction failed:", fallbackErr);
			}
		} finally {
			setFrameLoading(false);
		}
	}, []);

	useEffect(() => {
		if (opts.overlayVideoPath) {
			extractFrame(opts.overlayVideoPath);
		} else {
			setVideoMeta(null);
		}
	}, [opts.overlayVideoPath, extractFrame]);

	// ── File Dialogs ──────────────────────────────────────────────────────────
	const handleSelectJsonl = useCallback(async () => {
		try {
			const selected = await open({
				multiple: false,
				directory: false,
				filters: [{ name: "JSONL", extensions: ["jsonl"] }],
			});
			if (typeof selected === "string") onUpdate({ jsonFilePath: selected });
		} catch {}
	}, [onUpdate]);

	const handleSelectOutputFolder = useCallback(async () => {
		try {
			const selected = await open({ multiple: false, directory: true });
			if (typeof selected === "string") {
				const sep = selected.includes("\\") ? "\\" : "/";
				const path = selected.endsWith(sep)
					? `${selected}output.mp4`
					: `${selected}${sep}output.mp4`;
				setOpt("outputPath", path);
			}
		} catch {}
	}, [setOpt]);

	const handleSelectOverlayVideo = useCallback(async () => {
		try {
			const selected = await open({
				multiple: false,
				directory: false,
				filters: [
					{ name: "Video", extensions: ["mp4", "mov", "mkv", "avi", "webm"] },
				],
			});
			if (typeof selected === "string") {
				setOpt("overlayVideoPath", selected);
			}
		} catch {}
	}, [setOpt]);

	const handleBrowseImageOverlay = useCallback(
		async (index: number) => {
			try {
				const selected = await open({
					multiple: false,
					directory: false,
					filters: [
						{
							name: "Image",
							extensions: ["png", "jpg", "jpeg", "webp", "gif"],
						},
					],
				});
				if (typeof selected === "string") {
					const next = [...opts.imageOverlays];
					next[index] = { ...next[index], assetPath: selected };
					setOpt("imageOverlays", next);
				}
			} catch {}
		},
		[opts.imageOverlays, setOpt],
	);

	// ── Dispatch ──────────────────────────────────────────────────────────────
	const handleDispatch = useCallback(async () => {
		if (!tab.jsonFilePath || !opts.outputPath || isDispatching) return;
		setIsDispatching(true);
		setDispatchError(null);

		const taskId = crypto.randomUUID();
		try {
			await invoke("queue_chat_render", {
				id: taskId,
				jsonFilePath: tab.jsonFilePath,
				options: opts,
			});
			registerTaskSnapshot({
				tabId: tab.id,
				taskId,
				url: tab.url,
				jsonFilePath: tab.jsonFilePath,
				vodOptions: tab.vodOptions,
				chatOptions: tab.chatOptions,
				renderOptions: opts,
			});
			onUpdate({ activeTaskId: taskId });
			navigate({ to: "/queue" });
		} catch (e) {
			setDispatchError(e instanceof Error ? e.message : String(e));
		} finally {
			setIsDispatching(false);
		}
	}, [tab, opts, isDispatching, navigate, onUpdate, registerTaskSnapshot]);

	const canDispatch =
		Boolean(tab.jsonFilePath) && Boolean(opts.outputPath) && !isDispatching;

	// ─────────────────────────────────────────────────────────────────────────
	// Render
	// ─────────────────────────────────────────────────────────────────────────

	return (
		<div className="space-y-5 pb-4">

			{/* ── Source & Output ─────────────────────────────────────────────── */}
			<Section title="Source & Output">
				<div>
					<FieldLabel>Chat source (.jsonl)</FieldLabel>
					<BrowseInput
						value={tab.jsonFilePath ?? ""}
						onChange={(v) => onUpdate({ jsonFilePath: v })}
						onBrowse={handleSelectJsonl}
						placeholder="/path/to/chat.jsonl"
						browseLabel="Browse"
					/>
				</div>
				<div>
					<FieldLabel>Output path</FieldLabel>
					<BrowseInput
						value={opts.outputPath}
						onChange={(v) => setOpt("outputPath", v)}
						onBrowse={handleSelectOutputFolder}
						placeholder="/path/to/output.mp4"
						browseLabel="Browse"
					/>
				</div>
			</Section>

			{/* ── Canvas ──────────────────────────────────────────────────────── */}
			<Section title="Canvas">
				<div className="grid grid-cols-3 gap-2">
					<div>
						<FieldLabel>Width</FieldLabel>
						<NumberInput
							value={opts.width}
							min={1}
							onChange={(v) => setOpt("width", v)}
						/>
					</div>
					<div>
						<FieldLabel>Height</FieldLabel>
						<NumberInput
							value={opts.height}
							min={1}
							onChange={(v) => setOpt("height", v)}
						/>
					</div>
					<div>
						<FieldLabel>FPS</FieldLabel>
						<NumberInput
							value={opts.fps}
							min={1}
							max={120}
							onChange={(v) => setOpt("fps", v)}
						/>
					</div>
				</div>
			</Section>

			{/* ── Background ──────────────────────────────────────────────────── */}
			<Section title="Background">
				<PillTabs
					options={BG_MODES}
					value={opts.backgroundMode}
					onChange={(v) => setOpt("backgroundMode", v)}
				/>
				{opts.backgroundMode === "customColor" && (
					<ColorPicker
						label="Background color"
						value={opts.backgroundColor}
						onChange={(c) => setOpt("backgroundColor", c)}
					/>
				)}
			</Section>

			{/* ── Video Overlay & Visual Editor ───────────────────────────────── */}
			<Section title="Video Overlay">
				<div>
					<FieldLabel>Base video (optional)</FieldLabel>
					<BrowseInput
						value={opts.overlayVideoPath ?? ""}
						onChange={(v) => setOpt("overlayVideoPath", v || undefined)}
						onBrowse={handleSelectOverlayVideo}
						placeholder="/path/to/stream.mp4"
						browseLabel="Browse"
					/>
				</div>

				{opts.overlayVideoPath && (
					<div className="space-y-4 pt-1">
						<Toggle
							value={opts.useImmediatePipeOverlay}
							onChange={(v) => setOpt("useImmediatePipeOverlay", v)}
							label="Direct Pipe Mode (Zero-Copy)"
							description="Pass uncompressed GPU frames via FFmpeg stdin — bypasses temp files entirely"
						/>

						{/* Interactive canvas editor */}
						{frameLoading ? (
							<div className="w-full h-40 bg-neutral-900/50 border border-neutral-700/50 rounded-lg flex flex-col items-center justify-center gap-2 text-neutral-500">
								<span className="w-5 h-5 border-2 border-neutral-700 border-t-indigo-500 rounded-full animate-spin" />
								<span className="text-[11px]">Extracting frame…</span>
							</div>
						) : videoMeta ? (
							<CanvasEditor
								videoMeta={videoMeta}
								opts={opts}
								setOpt={setOpt}
							/>
						) : (
							<div className="w-full h-24 bg-neutral-900/40 border border-dashed border-neutral-700/50 rounded-lg flex items-center justify-center text-[11px] text-neutral-600">
								No preview available
							</div>
						)}

						<div className="pt-1 border-t border-neutral-800/60">
							<FieldLabel>Timeline mismatch strategy</FieldLabel>
							<SegmentedControl
								options={TIMELINE_STRATEGIES}
								value={opts.timelineMismatchStrategy}
								onChange={(v) => setOpt("timelineMismatchStrategy", v)}
							/>
						</div>
					</div>
				)}
			</Section>

			{/* ── Shape Overlays ───────────────────────────────────────────────── */}
			<Section title="Shape Overlays" defaultOpen={false}>
				<p className="text-[10px] text-neutral-600">
					Solid shapes drawn above the background, below chat. Drag them on the
					visual editor above.
				</p>
				<ShapeOverlayList
					overlays={opts.shapeOverlays}
					onChange={(v) => setOpt("shapeOverlays", v)}
				/>
			</Section>

			{/* ── Image Overlays ───────────────────────────────────────────────── */}
			<Section title="Image Overlays" defaultOpen={false}>
				<p className="text-[10px] text-neutral-600">
					Images composited above the background, below chat. Set a path then
					drag them to position.
				</p>
				<ImageOverlayList
					overlays={opts.imageOverlays}
					onChange={(v) => setOpt("imageOverlays", v)}
					onBrowse={handleBrowseImageOverlay}
				/>
			</Section>

			{/* ── Typography ──────────────────────────────────────────────────── */}
			<Section title="Typography" defaultOpen={false}>
				<div className="grid grid-cols-2 gap-2">
					<div>
						<FieldLabel>Font family</FieldLabel>
						<TextInput
							value={opts.fontName}
							onChange={(v) => setOpt("fontName", v)}
							placeholder="Inter"
						/>
					</div>
					<CustomSlider
						label="Font Size"
						value={opts.fontSize}
						min={8}
						max={72}
						unit="px"
						onChange={(v) => setOpt("fontSize", v)}
					/>
				</div>
				<div className="grid grid-cols-2 gap-2">
					<CustomSlider
						label="Line Spacing"
						value={opts.lineSpacing}
						min={0}
						max={40}
						unit="px"
						onChange={(v) => setOpt("lineSpacing", v)}
					/>
					<CustomSlider
						label="Message Spacing"
						value={opts.messageSpacing}
						min={0}
						max={60}
						unit="px"
						onChange={(v) => setOpt("messageSpacing", v)}
					/>
				</div>
			</Section>

			{/* ── Layout ───────────────────────────────────────────────────────── */}
			<Section title="Layout" defaultOpen={false}>
				<div className="grid grid-cols-2 gap-2">
					<CustomSlider
						label="Canvas Padding"
						value={opts.padding}
						min={0}
						max={80}
						unit="px"
						onChange={(v) => setOpt("padding", v)}
					/>
					<CustomSlider
						label="Bubble Padding"
						value={opts.bubblePadding}
						min={0}
						max={40}
						unit="px"
						onChange={(v) => setOpt("bubblePadding", v)}
					/>
				</div>
				<CustomSlider
					label="Bubble Radius"
					value={opts.bubbleRadius}
					min={0}
					max={32}
					unit="px"
					onChange={(v) => setOpt("bubbleRadius", v)}
				/>
				<div className="grid grid-cols-1 gap-2">
					<Toggle
						value={opts.bubbleModeFullWidth}
						onChange={(v) => setOpt("bubbleModeFullWidth", v)}
						label="Full-width bubbles"
						description="Stretch bubbles to canvas edge"
					/>
					<Toggle
						value={opts.centerEmotesVertically}
						onChange={(v) => setOpt("centerEmotesVertically", v)}
						label="Center emotes vertically"
					/>
				</div>
			</Section>

			{/* ── Colors ───────────────────────────────────────────────────────── */}
			<Section title="Colors" defaultOpen={false}>
				<ColorPicker
					label="Message text"
					value={opts.messageColor}
					onChange={(c) => setOpt("messageColor", c)}
				/>
				<ColorPicker
					label="Bubble"
					value={opts.bubbleColor}
					onChange={(c) => setOpt("bubbleColor", c)}
				/>
				<ColorPicker
					label="Highlight"
					value={opts.highlightColor}
					onChange={(c) => setOpt("highlightColor", c)}
				/>
			</Section>

			{/* ── Text Style ───────────────────────────────────────────────────── */}
			<Section title="Text Style" defaultOpen={false}>
				<Toggle
					value={opts.outlineUsernames}
					onChange={(v) => setOpt("outlineUsernames", v)}
					label="Outline usernames"
				/>
				{opts.outlineUsernames && (
					<CustomSlider
						label="Outline Width"
						value={opts.usernameOutlineWidth ?? 1.5}
						min={0.5}
						max={6}
						step={0.5}
						unit="px"
						onChange={(v) => setOpt("usernameOutlineWidth", v)}
					/>
				)}
				<Toggle
					value={opts.usernameShadow}
					onChange={(v) => setOpt("usernameShadow", v)}
					label="Username shadow"
				/>
			</Section>

			{/* ── Animations ───────────────────────────────────────────────────── */}
			<Section title="Animations" defaultOpen={false}>
				<Toggle
					value={opts.animSlide}
					onChange={(v) => setOpt("animSlide", v)}
					label="Slide in"
					description="Messages slide from the right"
				/>
				<Toggle
					value={opts.animFadeIn}
					onChange={(v) => setOpt("animFadeIn", v)}
					label="Fade in"
					description="Messages fade from transparent"
				/>
			</Section>

			{/* ── Lifecycle ────────────────────────────────────────────────────── */}
			<Section title="Message Lifecycle" defaultOpen={false}>
				<SegmentedControl
					options={EVICTION_MODES}
					value={opts.evictionStrategy}
					onChange={(v) => setOpt("evictionStrategy", v)}
				/>
				{opts.evictionStrategy === "timed" && (
					<div className="grid grid-cols-2 gap-2 pt-1">
						<CustomSlider
							label="Hold Duration"
							value={opts.messageHoldSeconds}
							min={1}
							max={60}
							unit="s"
							onChange={(v) => setOpt("messageHoldSeconds", v)}
						/>
						<CustomSlider
							label="Fade Out"
							value={opts.messageFadeOutSeconds}
							min={0}
							max={10}
							unit="s"
							onChange={(v) => setOpt("messageFadeOutSeconds", v)}
						/>
					</div>
				)}
			</Section>

			{/* ── User Filters ─────────────────────────────────────────────────── */}
			<Section title="User Filters" defaultOpen={false}>
				<div>
					<FieldLabel>Pinned users (comma-separated)</FieldLabel>
					<TextInput
						value={pinnedRaw}
						onChange={setPinnedRaw}
						onBlur={() => flushUserList("pinnedUsers", pinnedRaw)}
						placeholder="streamer, moderator"
					/>
				</div>
				<div>
					<FieldLabel>Skip users (comma-separated)</FieldLabel>
					<TextInput
						value={skipRaw}
						onChange={setSkipRaw}
						onBlur={() => flushUserList("skipUsers", skipRaw)}
						placeholder="BotRix, KickBot"
					/>
				</div>
				<CustomSlider
					label="Pin Duration"
					value={opts.pinDurationSecs}
					min={1}
					max={60}
					unit="s"
					onChange={(v) => setOpt("pinDurationSecs", v)}
				/>
			</Section>

			{/* ── Grouping ─────────────────────────────────────────────────────── */}
			<Section title="Message Grouping" defaultOpen={false}>
				<Toggle
					value={opts.groupMessages}
					onChange={(v) => setOpt("groupMessages", v)}
					label="Group consecutive messages"
					description="Messages from the same user within the window are merged"
				/>
				{opts.groupMessages && (
					<CustomSlider
						label="Grouping Window"
						value={opts.groupMessagesWindowSecs}
						min={1}
						max={30}
						unit="s"
						onChange={(v) => setOpt("groupMessagesWindowSecs", v)}
					/>
				)}
			</Section>

			{/* ── Quality ──────────────────────────────────────────────────────── */}
			<Section title="Quality" defaultOpen={false}>
				<SegmentedControl
					options={QUALITY_PRESETS}
					value={opts.qualityPreset}
					onChange={(v) => setOpt("qualityPreset", v)}
				/>
				<div>
					<FieldLabel>Max cached emotes</FieldLabel>
					<NumberInput
						value={opts.maxCachedEmotes}
						min={16}
						max={2048}
						onChange={(v) => setOpt("maxCachedEmotes", v)}
					/>
				</div>
				<Toggle
					value={opts.createPremultipliedAlphaEmotes}
					onChange={(v) => setOpt("createPremultipliedAlphaEmotes", v)}
					label="Premultiplied alpha emotes"
					description="Faster compositing — disable only if you see fringing"
				/>
			</Section>

			{/* ── Time Window ──────────────────────────────────────────────────── */}
			<Section title="Time Window" defaultOpen={false}>
				<p className="text-[10px] text-neutral-600">
					Leave blank to render the full log. Values are milliseconds from
					stream start.
				</p>
				<div className="grid grid-cols-3 gap-2">
					<div>
						<FieldLabel>Start (ms)</FieldLabel>
						<NumberInput
							value={opts.startMs ?? 0}
							min={0}
							onChange={(v) => setOpt("startMs", v || undefined)}
						/>
					</div>
					<div>
						<FieldLabel>End (ms)</FieldLabel>
						<NumberInput
							value={opts.endMs ?? 0}
							min={0}
							onChange={(v) => setOpt("endMs", v || undefined)}
						/>
					</div>
					<div>
						<FieldLabel>Zero point (ms)</FieldLabel>
						<NumberInput
							value={opts.timeZeroMs ?? 0}
							min={0}
							onChange={(v) => setOpt("timeZeroMs", v || undefined)}
						/>
					</div>
				</div>
			</Section>

			{/* ── Dispatch ─────────────────────────────────────────────────────── */}
			{dispatchError && (
				<div className="flex items-start gap-2 px-3 py-2.5 bg-red-950/40 border border-red-800/50 rounded-lg text-xs text-red-400">
					<svg
						className="flex-shrink-0 mt-0.5"
						width="12"
						height="12"
						viewBox="0 0 16 16"
						fill="currentColor"
					>
						<path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" />
					</svg>
					<span>
						<strong className="font-semibold">Dispatch failed: </strong>
						{dispatchError}
					</span>
				</div>
			)}

			<button
				type="button"
				onClick={handleDispatch}
				disabled={!canDispatch}
				className={`w-full py-3 text-sm font-bold rounded-xl transition-all tracking-wide ${
					canDispatch
						? "bg-violet-600 hover:bg-violet-500 text-white shadow-lg shadow-violet-900/40 active:scale-[0.99]"
						: "bg-neutral-800 text-neutral-600 cursor-not-allowed"
				}`}
			>
				{isDispatching ? (
					<span className="flex items-center justify-center gap-2">
						<span className="w-3.5 h-3.5 border-2 border-violet-400/40 border-t-white/80 rounded-full animate-spin" />
						Queuing…
					</span>
				) : (
					"Queue Render"
				)}
			</button>
		</div>
	);
}