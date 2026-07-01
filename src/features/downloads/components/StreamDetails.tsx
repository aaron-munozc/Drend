import { useState, useEffect } from "react";
import {
	Download,
	MessageSquare,
	Video,
	Settings2,
	FolderOpen,
	HardDrive,
	MonitorPlay,
	Activity,
	Scissors,
	Clock,
} from "lucide-react";
import { open, message } from "@tauri-apps/plugin-dialog";
import { DownloadTab } from "@/store/useDownloadTabStore.ts";
import {
	QualityPreference,
	VideoFormat,
} from "@/features/downloads/types/types.ts";
import { useQueueStore } from "@/store/useQueueStore.ts";

const formatTime = (totalSeconds: number): string => {
	const h = Math.floor(totalSeconds / 3600);
	const m = Math.floor((totalSeconds % 3600) / 60);
	const s = Math.floor(totalSeconds % 60);
	return [h, m, s].map((val) => val.toString().padStart(2, "0")).join(":");
};

const timeToMs = (timeStr: string): number | undefined => {
	const trimmed = timeStr.trim();
	if (!trimmed) return undefined;
	if (/^\d+$/.test(trimmed)) return parseInt(trimmed, 10) * 1000;

	const parts = trimmed.split(":").map(Number);
	if (parts.some(isNaN)) return undefined;

	let ms = 0;
	if (parts.length === 3) {
		ms += parts[0] * 3600000;
		ms += parts[1] * 60000;
		ms += parts[2] * 1000;
	} else if (parts.length === 2) {
		ms += parts[0] * 60000;
		ms += parts[1] * 1000;
	} else if (parts.length === 1) {
		ms += parts[0] * 1000;
	}
	return ms;
};

export function StreamDetails({ tab }: { tab: DownloadTab }) {
	const { enqueueTask } = useQueueStore();
	const payload = tab.metadata;

	// Read defaults from the cloned tab if they exist
	const initOpts = tab.initialOptions || {};
	const isInitialChat = tab.initialTaskType === "chatDownload";

	const initQuality = initOpts.quality
		? typeof initOpts.quality === "object"
			? initOpts.quality.index.toString()
			: initOpts.quality
		: "best";

	const initialStartSec = initOpts.startMs
		? Math.floor(initOpts.startMs / 1000)
		: 0;

	// Core States
	const [downloadMode, setDownloadMode] = useState<"vod" | "chat">(
		tab.initialTaskType ? (isInitialChat ? "chat" : "vod") : "vod",
	);
	const [showAdvanced, setShowAdvanced] = useState(
		Object.keys(initOpts).length > 0, // Auto-open if cloned parameters exist
	);

	// Form State Configurations
	const [selectedQuality, setSelectedQuality] = useState<string>(initQuality);
	const [selectedFormat, setSelectedFormat] = useState<VideoFormat>(
		initOpts.format || "mp4",
	);

	// Output Target States
	const [saveFolder, setSaveFolder] = useState(initOpts.saveFolder || "");
	const [fileName, setFileName] = useState(initOpts.fileName || "");

	// Advanced State Configurations
	const [threads, setThreads] = useState(initOpts.threads || 4);
	const [bufferMs, setBufferMs] = useState(initOpts.bufferMs || 2000);
	const [maxRetries, setMaxRetries] = useState(initOpts.maxRetries || 8);
	const [kickConcurrency, setKickConcurrency] = useState(
		initOpts.kickConcurrency || 10,
	);
	const [emptyCycleThreshold, setEmptyCycleThreshold] = useState(
		initOpts.emptyCycleThreshold || 6,
	);

	// Timeline States
	const rawDurationMs = payload?.streamMetadata?.duration || 0;
	const durationSec = Math.max(0, Math.floor(rawDurationMs / 1000));

	const [startSec, setStartSec] = useState(initialStartSec);
	const [endSec, setEndSec] = useState(
		initOpts.endMs ? Math.floor(initOpts.endMs / 1000) : durationSec > 0 ? durationSec : 0,
	);
	const [startTimeText, setStartTimeText] = useState(
		formatTime(initialStartSec),
	);
	const [endTimeText, setEndTimeText] = useState(
		initOpts.endMs
			? formatTime(Math.floor(initOpts.endMs / 1000))
			: durationSec > 0
				? formatTime(durationSec)
				: "",
	);

	useEffect(() => {
		if (durationSec > 0 && endSec === 0 && !initOpts.endMs) {
			setEndSec(durationSec);
			setEndTimeText(formatTime(durationSec));
		}
	}, [durationSec, endSec, initOpts.endMs]);

	const handleStartTextBlur = () => {
		const ms = timeToMs(startTimeText);
		if (ms !== undefined) {
			let sec = Math.floor(ms / 1000);
			sec = Math.max(0, Math.min(sec, endSec - 1));
			setStartSec(sec);
			setStartTimeText(formatTime(sec));
		} else setStartTimeText(formatTime(startSec));
	};

	const handleEndTextBlur = () => {
		const ms = timeToMs(endTimeText);
		if (ms !== undefined) {
			let sec = Math.floor(ms / 1000);
			if (durationSec > 0) sec = Math.min(sec, durationSec);
			sec = Math.max(startSec + 1, sec);
			setEndSec(sec);
			setEndTimeText(formatTime(sec));
		} else setEndTimeText(formatTime(endSec));
	};

	if (!payload) {
		return (
			<div className="flex h-full items-center justify-center text-muted-foreground p-6 animate-pulse">
				<Activity className="h-5 w-5 mr-3 animate-spin" />
				Waiting for stream payload to mount...
			</div>
		);
	}

	const { streamMetadata: meta, qualities } = payload;

	const handleSelectFolder = async () => {
		try {
			const selectedPath = await open({
				directory: true,
				multiple: false,
				title: "Select Destination Folder",
			});
			if (selectedPath && typeof selectedPath === "string")
				setSaveFolder(selectedPath);
		} catch (err) {
			console.error("Failed to open dialog", err);
		}
	};

	const handleQueueDownload = async () => {
		const finalStartMs =
			durationSec > 0 ? startSec * 1000 : timeToMs(startTimeText);
		const finalEndMs = durationSec > 0 ? endSec * 1000 : timeToMs(endTimeText);
		const targetTitle = meta.title || "Unknown Stream Session";

		if (downloadMode === "vod") {
			let qualityParam: QualityPreference | "best" | "worst" = "best";
			if (selectedQuality === "worst") qualityParam = "worst";
			else if (selectedQuality !== "best")
				qualityParam = { index: parseInt(selectedQuality, 10) };

			enqueueTask("vodDownload", targetTitle, {
				url: tab.url,
				options: {
					quality: qualityParam,
					format: selectedFormat,
					threads,
					startMs: finalStartMs,
					endMs: finalEndMs,
					bufferMs,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
				},
			});
		} else {
			enqueueTask("chatDownload", targetTitle, {
				url: tab.url,
				options: {
					startMs: finalStartMs,
					endMs: finalEndMs,
					bufferMs,
					maxRetries,
					kickConcurrency,
					emptyCycleThreshold,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
				},
			});
		}

		await message("Task successfully sent to the processing pipeline.", {
			title: "Queued",
			kind: "info",
		});
	};

	return (
		<div className="grid h-full grid-cols-1 lg:grid-cols-12 gap-8 p-6 overflow-y-auto bg-muted/10 text-foreground relative">
			{/* LEFT COLUMN: STICKY METADATA FRAME */}
			<div className="lg:col-span-5 relative h-max lg:sticky lg:top-0 space-y-4">
				<div className="overflow-hidden rounded-xl border border-border bg-card shadow-xs transition-all hover:shadow-md">
					<div className="relative aspect-video w-full bg-muted overflow-hidden group">
						{meta.thumbnailUrl ? (
							<img
								src={meta.thumbnailUrl}
								alt="VOD Content"
								className="h-full w-full object-cover transition-transform duration-700 group-hover:scale-105"
							/>
						) : (
							<div className="flex h-full w-full items-center justify-center">
								<MonitorPlay className="h-10 w-10 text-muted-foreground/30" />
							</div>
						)}
						<div className="absolute inset-0 bg-linear-to-t from-black/70 via-black/20 to-transparent" />
						<div className="absolute top-3 left-3 flex gap-2">
							<span className="inline-flex items-center rounded-md bg-primary px-2 py-1 text-[10px] font-bold text-primary-foreground uppercase tracking-widest shadow-sm">
								{meta.platform}
							</span>
							<span
								className={`inline-flex items-center rounded-md px-2 py-1 text-[10px] font-bold uppercase tracking-widest shadow-sm ${meta.streamStatus === "live" ? "bg-red-500 text-white animate-pulse" : "bg-secondary text-secondary-foreground"}`}
							>
								{meta.streamStatus || "Offline"}
							</span>
						</div>
						{durationSec > 0 && (
							<div className="absolute bottom-3 right-3 flex items-center gap-1.5 rounded-md bg-black/80 px-2 py-1 text-xs font-semibold text-white backdrop-blur-md">
								<Clock className="h-3 w-3" />
								{formatTime(durationSec)}
							</div>
						)}
					</div>

					<div className="p-5 space-y-5">
						<div className="space-y-1.5">
							<h2
								className="text-xl font-bold leading-tight tracking-tight line-clamp-2"
								title={meta.title}
							>
								{meta.title || "Untitled Stream Session"}
							</h2>
							<p className="text-sm font-medium text-muted-foreground flex items-center gap-2">
								<span className="h-5 w-5 rounded-full bg-muted-foreground/20 flex items-center justify-center text-[10px] uppercase font-bold text-foreground">
									{meta.username?.charAt(0) || "?"}
								</span>
								{meta.username || "Unknown Broadcaster"}
							</p>
						</div>
						<div className="grid grid-cols-2 gap-4 rounded-lg bg-muted/40 p-4 text-sm border border-border/50">
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-xs text-muted-foreground font-semibold uppercase tracking-wider">
									<Activity className="h-3.5 w-3.5" /> Views
								</span>
								<span className="font-semibold text-foreground">
									{meta.views?.toLocaleString() || "N/A"}
								</span>
							</div>
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-xs text-muted-foreground font-semibold uppercase tracking-wider">
									Followers
								</span>
								<span className="font-semibold text-foreground">
									{meta.followers?.toLocaleString() || "N/A"}
								</span>
							</div>
						</div>
					</div>
				</div>
			</div>

			{/* RIGHT COLUMN: PARAMETERS CONFIGURATION MATRIX */}
			<div className="lg:col-span-7 flex flex-col space-y-6 pb-12">
				<div className="rounded-xl border border-border bg-card p-1.5 shadow-xs flex items-center relative overflow-hidden">
					<button
						onClick={() => setDownloadMode("vod")}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-sm font-semibold transition-all duration-300 ease-out z-10 ${downloadMode === "vod" ? "bg-primary text-primary-foreground shadow-md scale-[0.98]" : "text-muted-foreground hover:text-foreground hover:bg-muted/50"}`}
					>
						<Video className="h-4 w-4" /> Video Stream (VOD)
					</button>
					<button
						onClick={() => setDownloadMode("chat")}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-sm font-semibold transition-all duration-300 ease-out z-10 ${downloadMode === "chat" ? "bg-primary text-primary-foreground shadow-md scale-[0.98]" : "text-muted-foreground hover:text-foreground hover:bg-muted/50"}`}
					>
						<MessageSquare className="h-4 w-4" /> Metadata Chat Log
					</button>
				</div>

				<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-xs transition-all hover:border-primary/20">
					<div className="flex items-center gap-2 border-b border-border pb-3">
						<Scissors className="h-4 w-4 text-primary" />
						<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
							Timeline Boundaries
						</h3>
					</div>

					<div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
						<div className="space-y-4 p-4 rounded-xl bg-muted/20 border border-border/50 hover:border-primary/40 transition-colors group">
							<div className="flex justify-between items-center">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-hover:text-primary transition-colors">
									Start Position
								</label>
								<span className="text-[10px] font-mono font-bold text-primary bg-primary/10 px-2 py-1 rounded-md">
									{formatTime(startSec)}
								</span>
							</div>
							{durationSec > 0 && (
								<input
									type="range"
									min="0"
									max={durationSec}
									value={startSec}
									onChange={(e) => {
										const val = Number(e.target.value);
										if (val < endSec) {
											setStartSec(val);
											setStartTimeText(formatTime(val));
										}
									}}
									className="w-full h-1.5 bg-border rounded-lg appearance-none cursor-grab active:cursor-grabbing accent-primary transition-all hover:h-2"
								/>
							)}
							<input
								type="text"
								value={startTimeText}
								onChange={(e) => setStartTimeText(e.target.value)}
								onBlur={handleStartTextBlur}
								placeholder="HH:MM:SS or Seconds"
								className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm shadow-xs transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 focus:border-primary/50 font-mono"
							/>
						</div>

						<div className="space-y-4 p-4 rounded-xl bg-muted/20 border border-border/50 hover:border-primary/40 transition-colors group">
							<div className="flex justify-between items-center">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-hover:text-primary transition-colors">
									End Position
								</label>
								<span className="text-[10px] font-mono font-bold text-primary bg-primary/10 px-2 py-1 rounded-md">
									{formatTime(endSec)}
								</span>
							</div>
							{durationSec > 0 && (
								<input
									type="range"
									min="0"
									max={durationSec}
									value={endSec}
									onChange={(e) => {
										const val = Number(e.target.value);
										if (val > startSec) {
											setEndSec(val);
											setEndTimeText(formatTime(val));
										}
									}}
									className="w-full h-1.5 bg-border rounded-lg appearance-none cursor-grab active:cursor-grabbing accent-primary transition-all hover:h-2"
								/>
							)}
							<input
								type="text"
								value={endTimeText}
								onChange={(e) => setEndTimeText(e.target.value)}
								onBlur={handleEndTextBlur}
								placeholder="HH:MM:SS or Seconds"
								className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm shadow-xs transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 focus:border-primary/50 font-mono"
							/>
						</div>
					</div>
				</div>

				{downloadMode === "vod" && (
					<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-xs transition-all hover:border-primary/20">
						<div className="flex items-center gap-2 border-b border-border pb-3">
							<Settings2 className="h-4 w-4 text-primary" />
							<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
								Encoding & Quality
							</h3>
						</div>
						<div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
							<div className="space-y-2 group">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
									Target Resolution
								</label>
								<select
									value={selectedQuality}
									onChange={(e) => setSelectedQuality(e.target.value)}
									className="w-full rounded-lg border border-input bg-background px-3 py-2.5 text-sm shadow-xs transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40 cursor-pointer"
								>
									<option value="best">Highest (Source Quality)</option>
									<option value="worst">Lowest Available</option>
									{qualities?.map((q) => (
										<option key={q.index} value={q.index.toString()}>
											{q.resolution
												? `${q.resolution.width}x${q.resolution.height}`
												: `Stream Profile ${q.index}`}
											{q.bandwidth
												? ` (${Math.round(q.bandwidth / 1000)} Kbps)`
												: ""}
										</option>
									))}
								</select>
							</div>
							<div className="space-y-2 group">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
									Container Format
								</label>
								<select
									value={selectedFormat}
									onChange={(e) =>
										setSelectedFormat(e.target.value as VideoFormat)
									}
									className="w-full rounded-lg border border-input bg-background px-3 py-2.5 text-sm shadow-xs transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40 cursor-pointer"
								>
									<option value="mp4">MPEG-4 (.mp4)</option>
									<option value="mkv">Matroska (.mkv)</option>
									<option value="ts">Transport Stream (.ts)</option>
								</select>
							</div>
						</div>
					</div>
				)}

				<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-xs transition-all hover:border-primary/20">
					<div className="flex items-center gap-2 border-b border-border pb-3">
						<HardDrive className="h-4 w-4 text-primary" />
						<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
							Output Destination
						</h3>
					</div>
					<div className="space-y-5 pt-2">
						<div className="space-y-2 group">
							<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
								Save Directory
							</label>
							<div className="flex items-center gap-3">
								<div className="flex-1 rounded-lg border border-input bg-muted/30 px-3 py-2.5 text-sm text-muted-foreground shadow-inner truncate transition-colors hover:bg-muted/50 cursor-default">
									{saveFolder || "System Default Downloads Folder"}
								</div>
								<button
									onClick={handleSelectFolder}
									className="flex items-center gap-2 rounded-lg bg-secondary px-5 py-2.5 text-sm font-semibold text-secondary-foreground transition-all hover:bg-secondary/80 hover:shadow-md active:scale-95 shrink-0"
								>
									<FolderOpen className="h-4 w-4" />
									Browse...
								</button>
							</div>
						</div>
						<div className="space-y-2 group">
							<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
								Custom Filename (Optional)
							</label>
							<input
								type="text"
								value={fileName}
								onChange={(e) => setFileName(e.target.value)}
								placeholder="e.g. tournament_finals_2026"
								className="w-full rounded-lg border border-input bg-background px-3 py-2.5 text-sm shadow-xs transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
							/>
						</div>
					</div>
				</div>

				<div className="rounded-xl border border-border bg-card overflow-hidden shadow-xs transition-all hover:border-primary/20">
					<button
						onClick={() => setShowAdvanced(!showAdvanced)}
						className="flex w-full items-center justify-between bg-muted/20 px-6 py-4 transition-all hover:bg-muted/50 active:bg-muted/60"
					>
						<span className="flex items-center gap-2 text-sm font-bold uppercase tracking-wider text-foreground">
							<Settings2 className="h-4 w-4 text-muted-foreground" />
							Engine Runtime Flags
						</span>
						<span className="text-xs font-semibold text-muted-foreground bg-background border border-border px-3 py-1 rounded-md shadow-xs transition-colors hover:text-foreground">
							{showAdvanced ? "Hide Controls" : "Configure"}
						</span>
					</button>
					{showAdvanced && (
						<div className="p-6 border-t border-border bg-background/30 grid grid-cols-1 md:grid-cols-2 gap-6 animate-in slide-in-from-top-2 fade-in duration-200">
							<div className="space-y-2 group">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
									Buffer Size (ms)
								</label>
								<input
									type="number"
									value={bufferMs}
									onChange={(e) => setBufferMs(Number(e.target.value))}
									min={500}
									className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
								/>
							</div>
							{downloadMode === "vod" ? (
								<div className="space-y-2 group">
									<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
										Worker Threads
									</label>
									<input
										type="number"
										value={threads}
										onChange={(e) => setThreads(Number(e.target.value))}
										min={1}
										max={32}
										className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
									/>
								</div>
							) : (
								<>
									<div className="space-y-2 group">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
											Max Network Retries
										</label>
										<input
											type="number"
											value={maxRetries}
											onChange={(e) => setMaxRetries(Number(e.target.value))}
											min={1}
											className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
										/>
									</div>
									<div className="space-y-2 group">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
											Kick Concurrency
										</label>
										<input
											type="number"
											value={kickConcurrency}
											onChange={(e) =>
												setKickConcurrency(Number(e.target.value))
											}
											min={1}
											className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
										/>
									</div>
									<div className="space-y-2 group">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider group-focus-within:text-primary transition-colors">
											Empty Cycle Limit
										</label>
										<input
											type="number"
											value={emptyCycleThreshold}
											onChange={(e) =>
												setEmptyCycleThreshold(Number(e.target.value))
											}
											min={1}
											className="w-full rounded-lg border border-input bg-background px-3 py-2 text-sm transition-all focus:outline-hidden focus:ring-2 focus:ring-primary/50 hover:border-primary/40"
										/>
									</div>
								</>
							)}
						</div>
					)}
				</div>

				<div className="pt-4">
					<button
						onClick={handleQueueDownload}
						className="group flex w-full items-center justify-center gap-3 rounded-xl bg-primary px-4 py-4 text-sm font-bold uppercase tracking-wider text-primary-foreground shadow-lg shadow-primary/20 transition-all hover:bg-primary/90 hover:shadow-xl hover:shadow-primary/30 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2 active:scale-[0.98]"
					>
						<Download className="h-5 w-5 transition-transform group-hover:-translate-y-0.5" />{" "}
						Send to Pipeline
					</button>
				</div>
			</div>
		</div>
	);
}