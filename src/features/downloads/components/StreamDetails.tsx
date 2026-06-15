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
} from "lucide-react";
import { open, message } from "@tauri-apps/plugin-dialog";
import { Tab } from "@/store/useTabStore.ts";
import {
	QualityPreference,
	VideoFormat,
} from "@/features/downloads/types/types.ts";
import { useDownloader } from "@/features/downloads/hooks/useDownlaoder.ts";

// Utility to format seconds into HH:MM:SS
const formatTime = (totalSeconds: number): string => {
	const h = Math.floor(totalSeconds / 3600);
	const m = Math.floor((totalSeconds % 3600) / 60);
	const s = Math.floor(totalSeconds % 60);
	return [h, m, s].map((val) => val.toString().padStart(2, "0")).join(":");
};

const timeToMs = (timeStr: string): number | undefined => {
	if (!timeStr.trim()) return undefined;
	const parts = timeStr.split(":").map(Number);
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

export function StreamDetails({ tab }: { tab: Tab }) {
	const { downloadVod, downloadChat } = useDownloader();
	const payload = tab.metadata;

	// Core States
	const [downloadMode, setDownloadMode] = useState<"vod" | "chat">("vod");
	const [showAdvanced, setShowAdvanced] = useState(false);

	// Form State Configurations
	const [selectedQuality, setSelectedQuality] = useState<string>("best");
	const [selectedFormat, setSelectedFormat] = useState<VideoFormat>("mp4");

	// Output Target States
	const [saveFolder, setSaveFolder] = useState("");
	const [fileName, setFileName] = useState("");

	// Advanced State Configurations
	const [threads, setThreads] = useState(4);
	const [bufferMs, setBufferMs] = useState(2000);
	const [maxRetries, setMaxRetries] = useState(8);
	const [kickConcurrency, setKickConcurrency] = useState(10);
	const [emptyCycleThreshold, setEmptyCycleThreshold] = useState(6);

	// Timeline States (derived from duration if available)
	const durationSec = payload?.streamMetadata?.duration || 0;
	const [startSec, setStartSec] = useState(0);
	const [endSec, setEndSec] = useState(durationSec > 0 ? durationSec : 0);

	// Sync text inputs with sliders safely
	const [startTimeText, setStartTimeText] = useState("00:00:00");
	const [endTimeText, setEndTimeText] = useState(
		durationSec > 0 ? formatTime(durationSec) : "",
	);

	// Update text boxes when sliders move
	useEffect(() => {
		setStartTimeText(formatTime(startSec));
	}, [startSec]);

	useEffect(() => {
		if (durationSec > 0) setEndTimeText(formatTime(endSec));
	}, [endSec, durationSec]);

	if (!payload || !payload.streamMetadata) {
		return (
			<div className="flex h-full items-center justify-center text-muted-foreground p-6">
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
			if (selectedPath && typeof selectedPath === "string") {
				setSaveFolder(selectedPath);
			}
		} catch (err) {
			console.error("Failed to open dialog", err);
		}
	};

	const handleQueueDownload = async () => {
		const finalStartMs =
			durationSec > 0 ? startSec * 1000 : timeToMs(startTimeText);
		const finalEndMs = durationSec > 0 ? endSec * 1000 : timeToMs(endTimeText);

		try {
			if (downloadMode === "vod") {
				let qualityParam: QualityPreference | "best" | "worst" = "best";
				if (selectedQuality === "worst") qualityParam = "worst";
				else if (selectedQuality !== "best") {
					qualityParam = { index: parseInt(selectedQuality, 10) };
				}

				await downloadVod(tab.url, {
					quality: qualityParam,
					format: selectedFormat,
					threads,
					startMs: finalStartMs,
					endMs: finalEndMs,
					bufferMs,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
				});
			} else {
				await downloadChat(tab.url, {
					startMs: finalStartMs,
					endMs: finalEndMs,
					bufferMs,
					maxRetries,
					kickConcurrency,
					emptyCycleThreshold,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
				});
			}
			await message(
				"Task successfully dispatched to background processor pipeline.",
				{ title: "Queue Initialized", kind: "info" },
			);
		} catch (err: any) {
			await message(`Pipeline routing failure: ${err.toString()}`, {
				title: "Execution Error",
				kind: "error",
			});
		}
	};

	return (
		<div className="grid h-full grid-cols-1 lg:grid-cols-12 gap-8 p-6 overflow-y-auto bg-muted/20 text-foreground relative">
			{/* LEFT COLUMN: STICKY METADATA FRAME */}
			<div className="lg:col-span-5 relative h-max lg:sticky lg:top-0 space-y-4">
				<div className="overflow-hidden rounded-xl border border-border bg-card shadow-sm">
					{/* Thumbnail Header with Absolute Badges */}
					<div className="relative aspect-video w-full bg-muted">
						{meta.thumbnailUrl ? (
							<img
								src={meta.thumbnailUrl}
								alt="VOD Content"
								className="h-full w-full object-cover"
							/>
						) : (
							<div className="flex h-full w-full items-center justify-center">
								<MonitorPlay className="h-10 w-10 text-muted-foreground/30" />
							</div>
						)}
						{/* Gradient Overlay for Text Readability */}
						<div className="absolute inset-0 bg-gradient-to-t from-black/60 via-transparent to-transparent" />

						{/* Floating Badges */}
						<div className="absolute top-3 left-3 flex gap-2">
							<span className="inline-flex items-center rounded-md bg-primary px-2 py-1 text-[10px] font-bold text-primary-foreground uppercase tracking-widest shadow-sm">
								{meta.platform}
							</span>
							<span
								className={`inline-flex items-center rounded-md px-2 py-1 text-[10px] font-bold uppercase tracking-widest shadow-sm ${
									meta.streamStatus === "live"
										? "bg-red-500 text-white animate-pulse"
										: "bg-secondary text-secondary-foreground"
								}`}
							>
								{meta.streamStatus || "Offline"}
							</span>
						</div>
						{durationSec > 0 && (
							<div className="absolute bottom-3 right-3 rounded-md bg-black/80 px-2 py-1 text-xs font-semibold text-white backdrop-blur-md">
								{formatTime(durationSec)}
							</div>
						)}
					</div>

					{/* Meta Info Body */}
					<div className="p-5 space-y-5">
						<div className="space-y-1.5">
							<h2
								className="text-xl font-bold leading-tight tracking-tight line-clamp-2"
								title={meta.title}
							>
								{meta.title || "Untitled Stream Session"}
							</h2>
							<p className="text-sm font-medium text-muted-foreground flex items-center gap-1.5">
								<div className="h-5 w-5 rounded-full bg-muted flex items-center justify-center text-[10px] uppercase font-bold text-foreground">
									{meta.username?.charAt(0) || "?"}
								</div>
								{meta.username || "Unknown Broadcaster"}
							</p>
						</div>

						<div className="grid grid-cols-2 gap-4 rounded-lg bg-muted/40 p-4 text-sm">
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-xs text-muted-foreground font-semibold uppercase tracking-wider">
									<Activity className="h-3.5 w-3.5" /> Views
								</span>
								<span className="font-semibold">
									{meta.views?.toLocaleString() || "N/A"}
								</span>
							</div>
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-xs text-muted-foreground font-semibold uppercase tracking-wider">
									Users
								</span>
								<span className="font-semibold">
									{meta.followers?.toLocaleString() || "N/A"}
								</span>
							</div>
						</div>
					</div>
				</div>
			</div>

			{/* RIGHT COLUMN: PARAMETERS CONFIGURATION MATRIX */}
			<div className="lg:col-span-7 flex flex-col space-y-6 pb-12">
				{/* PIPELINE TARGET SELECTOR (Segmented Control) */}
				<div className="rounded-xl border border-border bg-card p-1.5 shadow-sm flex items-center">
					<button
						onClick={() => setDownloadMode("vod")}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-sm font-semibold transition-all ${
							downloadMode === "vod"
								? "bg-primary text-primary-foreground shadow-md"
								: "text-muted-foreground hover:text-foreground hover:bg-muted/50"
						}`}
					>
						<Video className="h-4 w-4" />
						Video Stream (VOD)
					</button>
					<button
						onClick={() => setDownloadMode("chat")}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-sm font-semibold transition-all ${
							downloadMode === "chat"
								? "bg-primary text-primary-foreground shadow-md"
								: "text-muted-foreground hover:text-foreground hover:bg-muted/50"
						}`}
					>
						<MessageSquare className="h-4 w-4" />
						Metadata Chat Log
					</button>
				</div>

				{/* TIMELINE BOUNDARIES */}
				<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-sm">
					<div className="flex items-center gap-2 border-b border-border pb-3">
						<Scissors className="h-4 w-4 text-primary" />
						<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
							Timeline Boundaries
						</h3>
					</div>

					{durationSec > 0 ? (
						<div className="space-y-6 pt-2">
							{/* Visual Slider representations */}
							<div className="space-y-3">
								<div className="flex justify-between text-xs font-semibold text-muted-foreground">
									<span>Start: {startTimeText}</span>
									<span>End: {endTimeText}</span>
								</div>

								{/* Start Range */}
								<div className="space-y-1">
									<input
										type="range"
										min="0"
										max={durationSec}
										value={startSec}
										onChange={(e) => {
											const val = Number(e.target.value);
											if (val < endSec) setStartSec(val);
										}}
										className="w-full h-2 bg-muted rounded-lg appearance-none cursor-pointer accent-primary"
									/>
								</div>
								{/* End Range */}
								<div className="space-y-1">
									<input
										type="range"
										min="0"
										max={durationSec}
										value={endSec}
										onChange={(e) => {
											const val = Number(e.target.value);
											if (val > startSec) setEndSec(val);
										}}
										className="w-full h-2 bg-muted rounded-lg appearance-none cursor-pointer accent-primary"
									/>
								</div>
							</div>
						</div>
					) : (
						<div className="grid grid-cols-1 md:grid-cols-2 gap-4 pt-2">
							<div className="space-y-1.5">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
									Start Offset
								</label>
								<input
									type="text"
									value={startTimeText}
									onChange={(e) => setStartTimeText(e.target.value)}
									placeholder="00:00:00"
									className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50 font-mono"
								/>
							</div>
							<div className="space-y-1.5">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
									End Offset
								</label>
								<input
									type="text"
									value={endTimeText}
									onChange={(e) => setEndTimeText(e.target.value)}
									placeholder="01:30:00"
									className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50 font-mono"
								/>
							</div>
						</div>
					)}
				</div>

				{/* VOD ENCODING PARAMETERS */}
				{downloadMode === "vod" && (
					<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-sm">
						<div className="flex items-center gap-2 border-b border-border pb-3">
							<Settings2 className="h-4 w-4 text-primary" />
							<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
								Encoding & Quality
							</h3>
						</div>

						<div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
							<div className="space-y-2">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
									Target Resolution
								</label>
								<select
									value={selectedQuality}
									onChange={(e) => setSelectedQuality(e.target.value)}
									className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
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

							<div className="space-y-2">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
									Container Format
								</label>
								<select
									value={selectedFormat}
									onChange={(e) =>
										setSelectedFormat(e.target.value as VideoFormat)
									}
									className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
								>
									<option value="mp4">MPEG-4 (.mp4)</option>
									<option value="mkv">Matroska (.mkv)</option>
									<option value="ts">Transport Stream (.ts)</option>
								</select>
							</div>
						</div>
					</div>
				)}

				{/* OUTPUT DESTINATION */}
				<div className="space-y-5 rounded-xl border border-border bg-card p-6 shadow-sm">
					<div className="flex items-center gap-2 border-b border-border pb-3">
						<HardDrive className="h-4 w-4 text-primary" />
						<h3 className="text-sm font-bold tracking-wide uppercase text-foreground">
							Output Destination
						</h3>
					</div>

					<div className="space-y-5 pt-2">
						<div className="space-y-2">
							<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
								Save Directory
							</label>
							<div className="flex items-center gap-3">
								<div className="flex-1 rounded-md border border-input bg-muted/30 px-3 py-2 text-sm text-muted-foreground shadow-sm truncate">
									{saveFolder || "System Default Downloads Folder"}
								</div>
								<button
									onClick={handleSelectFolder}
									className="flex items-center gap-2 rounded-md bg-secondary px-4 py-2 text-sm font-semibold text-secondary-foreground hover:bg-secondary/80 transition-colors shadow-sm shrink-0"
								>
									<FolderOpen className="h-4 w-4" />
									Browse...
								</button>
							</div>
						</div>

						<div className="space-y-2">
							<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
								Custom Filename (Optional)
							</label>
							<input
								type="text"
								value={fileName}
								onChange={(e) => setFileName(e.target.value)}
								placeholder="e.g. tournament_finals_2026"
								className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
							/>
						</div>
					</div>
				</div>

				{/* ADVANCED ENGINE FLAGS ACCORDION */}
				<div className="rounded-xl border border-border bg-card overflow-hidden shadow-sm transition-all">
					<button
						onClick={() => setShowAdvanced(!showAdvanced)}
						className="flex w-full items-center justify-between bg-muted/30 px-6 py-4 hover:bg-muted/50 transition-colors"
					>
						<span className="flex items-center gap-2 text-sm font-bold uppercase tracking-wider text-foreground">
							<Settings2 className="h-4 w-4 text-muted-foreground" />
							Engine Runtime Flags
						</span>
						<span className="text-xs font-semibold text-muted-foreground bg-background border border-border px-2 py-1 rounded-md">
							{showAdvanced ? "Hide" : "Configure"}
						</span>
					</button>

					{showAdvanced && (
						<div className="p-6 border-t border-border bg-background/50 grid grid-cols-1 md:grid-cols-2 gap-6 animate-fadeIn">
							<div className="space-y-2">
								<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
									Buffer Size (ms)
								</label>
								<input
									type="number"
									value={bufferMs}
									onChange={(e) => setBufferMs(Number(e.target.value))}
									min={500}
									className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
								/>
							</div>

							{downloadMode === "vod" ? (
								<div className="space-y-2">
									<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
										Worker Threads
									</label>
									<input
										type="number"
										value={threads}
										onChange={(e) => setThreads(Number(e.target.value))}
										min={1}
										max={32}
										className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
									/>
								</div>
							) : (
								<>
									<div className="space-y-2">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
											Max Network Retries
										</label>
										<input
											type="number"
											value={maxRetries}
											onChange={(e) => setMaxRetries(Number(e.target.value))}
											min={1}
											className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
										/>
									</div>
									<div className="space-y-2">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
											Kick Concurrency
										</label>
										<input
											type="number"
											value={kickConcurrency}
											onChange={(e) =>
												setKickConcurrency(Number(e.target.value))
											}
											min={1}
											className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
										/>
									</div>
									<div className="space-y-2">
										<label className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
											Empty Cycle Limit
										</label>
										<input
											type="number"
											value={emptyCycleThreshold}
											onChange={(e) =>
												setEmptyCycleThreshold(Number(e.target.value))
											}
											min={1}
											className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-hidden focus:ring-2 focus:ring-primary/50"
										/>
									</div>
								</>
							)}
						</div>
					)}
				</div>

				{/* DISPATCH EXECUTION TRIGGER */}
				<div className="pt-2">
					<button
						onClick={handleQueueDownload}
						className="flex w-full items-center justify-center gap-3 rounded-xl bg-primary px-4 py-4 text-sm font-bold uppercase tracking-wider text-primary-foreground shadow-lg shadow-primary/20 transition-all hover:bg-primary/90 hover:shadow-primary/40 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2 active:scale-[0.98]"
					>
						<Download className="h-5 w-5" />
						Initialize Worker Pipeline
					</button>
				</div>
			</div>
		</div>
	);
}
