import { useState, useEffect, useMemo } from "react";
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
	Layers,
	Sparkles,
	Globe,
	Eye,
	ThumbsUp,
	Bookmark,
	Volume2,
} from "lucide-react";
import { open, message } from "@tauri-apps/plugin-dialog";
import { motion, AnimatePresence } from "framer-motion";
import { VideoFormat, AudioFormat } from "@/features/downloads/types/types.ts";
import { useQueueStore } from "@/store/useQueueStore.ts";
import {WorkspaceTab} from "@/store/useWorkspaceStore.ts";

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

export function UrlDetails({ tab }: { tab: WorkspaceTab }) {
	const { enqueueTask } = useQueueStore();
	const payload = tab.metadata;

	const initOpts = tab.initialOptions || {};
	const isInitialChat = tab.initialTaskType === "chatDownload";

	const [downloadMode, setDownloadMode] = useState<"vod" | "chat">(
		tab.initialTaskType ? (isInitialChat ? "chat" : "vod") : "vod",
	);
	const [showAdvanced, setShowAdvanced] = useState(
		Object.keys(initOpts).length > 0,
	);

	// Basic Tracking States
	const [saveFolder, setSaveFolder] = useState(initOpts.saveFolder || "");
	const [fileName, setFileName] = useState(initOpts.fileName || "");
	const [threads, setThreads] = useState(initOpts.threads || 4);

	// Extended Quality & Track Controls
	const [trackSelectionMode, setTrackSelectionMode] = useState<"unified" | "split">("unified");
	const [selectedFormatId, setSelectedFormatId] = useState<string>("best");
	const [videoFormatId, setVideoFormatId] = useState<string>("");
	const [audioFormatId, setAudioFormatId] = useState<string>("");

	const [audioOnly, setAudioOnly] = useState<boolean>(initOpts.audioOnly || false);
	const [selectedOutputContainer, setSelectedOutputContainer] = useState<VideoFormat>(initOpts.videoFormat || "mp4");
	const [audioFormat, setAudioFormat] = useState<AudioFormat>(initOpts.audioFormat || "best");

	// Extended yt-dlp Operational Flags
	const [limitRate, setLimitRate] = useState<string>(initOpts.limitRate || "");
	const [cookiesBrowser, setCookiesBrowser] = useState<string>(initOpts.cookiesBrowser || "");
	const [forceKeyframes, setForceKeyframes] = useState<boolean>(initOpts.forceKeyframes || false);
	const [liveFromStart, setLiveFromStart] = useState<boolean>(initOpts.liveFromStart || false);
	const [sponsorblock, setSponsorblock] = useState<boolean>(initOpts.sponsorblock || false);

	// Embedding Options
	const [embedMetadata, setEmbedMetadata] = useState<boolean>(initOpts.embedMetadata ?? true);
	const [embedThumbnail, setEmbedThumbnail] = useState<boolean>(initOpts.embedThumbnail ?? true);
	const [embedChapters, setEmbedChapters] = useState<boolean>(initOpts.embedChapters ?? true);
	const [embedSubs, setEmbedSubs] = useState<boolean>(initOpts.embedSubs || false);
	const [writeAutoSubs, setWriteAutoSubs] = useState<boolean>(initOpts.writeAutoSubs || false);

	// Chat Engine Options
	const [maxRetries, setMaxRetries] = useState(initOpts.maxRetries || 8);
	const [kickConcurrency, setKickConcurrency] = useState(initOpts.kickConcurrency || 10);
	const [emptyCycleThreshold, setEmptyCycleThreshold] = useState(initOpts.emptyCycleThreshold || 6);

	// Timeline Metrics
	const durationSec = useMemo(() => {
		if (!payload?.normalized?.duration) return 0;
		return Math.max(0, Math.floor(payload.normalized.duration));
	}, [payload?.normalized?.duration]);

	const initialStartSec = initOpts.startMs ? Math.floor(initOpts.startMs / 1000) : 0;
	const [startSec, setStartSec] = useState(initialStartSec);
	const [endSec, setEndSec] = useState(
		initOpts.endMs ? Math.floor(initOpts.endMs / 1000) : durationSec > 0 ? durationSec : 0
	);
	const [startTimeText, setStartTimeText] = useState(formatTime(initialStartSec));
	const [endTimeText, setEndTimeText] = useState(
		initOpts.endMs ? formatTime(Math.floor(initOpts.endMs / 1000)) : durationSec > 0 ? formatTime(durationSec) : ""
	);

	const availableFormats = payload?.normalized?.formats || [];
	const preMergedFormats = useMemo(() => availableFormats.filter((f) => f.hasVideo && f.hasAudio), [availableFormats]);
	const videoOnlyFormats = useMemo(() => availableFormats.filter((f) => f.hasVideo && !f.hasAudio), [availableFormats]);
	const audioOnlyFormats = useMemo(() => availableFormats.filter((f) => !f.hasVideo && f.hasAudio), [availableFormats]);

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

	const snapToTimeline = (start: number, end: number) => {
		const parsedStart = Math.max(0, Math.floor(start));
		const parsedEnd = durationSec > 0 ? Math.min(Math.floor(end), durationSec) : Math.floor(end);
		setStartSec(parsedStart);
		setEndSec(parsedEnd);
		setStartTimeText(formatTime(parsedStart));
		setEndTimeText(formatTime(parsedEnd));
	};

	if (!payload) {
		return (
			<div className="flex h-full flex-col items-center justify-center text-muted-foreground p-6 space-y-4">
				<Activity className="h-8 w-8 animate-spin text-primary" />
				<p className="text-sm font-medium tracking-wide">Assembling dynamic yt-dlp metadata profile...</p>
			</div>
		);
	}

	const meta = payload.normalized;
	const isChatSupported = meta.isChatSupported;

	const handleSelectFolder = async () => {
		try {
			const selectedPath = await open({
				directory: true,
				multiple: false,
				title: "Select Destination Folder",
			});
			if (selectedPath && typeof selectedPath === "string") setSaveFolder(selectedPath);
		} catch (err) {
			console.error("Failed to map save target", err);
		}
	};

	const handleQueueDownload = async () => {
		const finalStartMs = durationSec > 0 ? startSec * 1000 : timeToMs(startTimeText);
		const finalEndMs = durationSec > 0 ? endSec * 1000 : timeToMs(endTimeText);
		const targetTitle = meta.title || "Unknown Stream Session";

		if (downloadMode === "vod") {
			let outVideoId: string | undefined;
			let outAudioId: string | undefined;

			if (trackSelectionMode === "unified") {
				if (selectedFormatId !== "best" && selectedFormatId !== "worst") {
					const target = availableFormats.find((f) => f.formatId === selectedFormatId);
					if (target) {
						if (target.hasVideo && target.hasAudio) {
							outVideoId = target.formatId;
						} else if (target.hasVideo) {
							outVideoId = target.formatId;
							outAudioId = "best";
						} else {
							outAudioId = target.formatId;
						}
					}
				} else {
					outVideoId = selectedFormatId;
				}
			} else {
				outVideoId = videoFormatId.trim() ? videoFormatId : undefined;
				outAudioId = audioFormatId.trim() ? audioFormatId : undefined;
			}

			enqueueTask("vodDownload", targetTitle, {
				url: tab.url,
				options: {
					videoFormatId: outVideoId,
					audioFormatId: outAudioId,
					audioOnly,
					videoFormat: selectedOutputContainer,
					audioFormat: audioOnly ? audioFormat : undefined,
					threads,
					startMs: finalStartMs,
					endMs: finalEndMs,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
					forceKeyframes,
					limitRate: limitRate.trim() ? limitRate : undefined,
					cookiesBrowser: cookiesBrowser.trim() ? cookiesBrowser : undefined,
					liveFromStart,
					embedMetadata,
					embedThumbnail,
					embedChapters,
					embedSubs,
					writeAutoSubs,
					sponsorblock,
				},
			});
		} else {
			enqueueTask("chatDownload", targetTitle, {
				url: tab.url,
				options: {
					startMs: finalStartMs,
					endMs: finalEndMs,
					maxRetries,
					kickConcurrency,
					emptyCycleThreshold,
					saveFolder: saveFolder.trim() ? saveFolder : undefined,
					fileName: fileName.trim() ? fileName : undefined,
				},
			});
		}

		await message("Target pipeline tasks committed into ingestion manager successfully.", {
			title: "Pipeline Instantiated",
			kind: "info",
		});
	};

	return (
		<motion.div
			initial={{ opacity: 0, y: 4 }}
			animate={{ opacity: 1, y: 0 }}
			className="grid h-full grid-cols-1 xl:grid-cols-12 gap-8 p-6 overflow-y-auto bg-background/40 backdrop-blur-sm text-foreground relative"
		>
			{/* LEFT COLUMN: Media Preview Card & Chapter Segmentation */}
			<div className="xl:col-span-5 relative h-max xl:sticky xl:top-0 space-y-6">
				<div className="overflow-hidden rounded-2xl border border-border/80 bg-card/60 shadow-xl backdrop-blur-md">
					<div className="relative aspect-video w-full bg-muted overflow-hidden group">
						{meta.thumbnail ? (
							<img
								src={meta.thumbnail}
								alt="VOD Stream Canvas"
								className="h-full w-full object-cover transition-transform duration-500 group-hover:scale-[1.02]"
							/>
						) : (
							<div className="flex h-full w-full items-center justify-center">
								<MonitorPlay className="h-12 w-12 text-muted-foreground/20" />
							</div>
						)}
						<div className="absolute inset-0 bg-gradient-to-t from-background via-black/10 to-transparent" />

						<div className="absolute top-4 left-4 flex gap-2">
							<span className="inline-flex items-center rounded-lg bg-primary px-2.5 py-1 text-[10px] font-bold text-primary-foreground uppercase tracking-wider shadow-md">
								{meta.extractor || "Generic Indexer"}
							</span>
							<span
								className={`inline-flex items-center rounded-lg px-2.5 py-1 text-[10px] font-bold uppercase tracking-wider shadow-md ${
									meta.isLive ? "bg-red-500 text-white animate-pulse" : "bg-muted/80 text-muted-foreground border border-border"
								}`}
							>
								{meta.isLive ? "Live" : meta.wasLive ? "Archive VOD" : "Static Content"}
							</span>
						</div>

						{durationSec > 0 && (
							<div className="absolute bottom-4 right-4 flex items-center gap-1.5 rounded-lg bg-background/90 border border-border px-2.5 py-1 text-xs font-mono font-bold shadow-md">
								<Clock className="h-3.5 w-3.5 text-primary" />
								{formatTime(durationSec)}
							</div>
						)}
					</div>

					<div className="p-6 space-y-6">
						<div className="space-y-2">
							<h2 className="text-xl font-extrabold leading-snug tracking-tight line-clamp-2" title={meta.title}>
								{meta.title}
							</h2>
							<div className="flex items-center gap-2 pt-1">
								<div className="h-6 w-6 rounded-md bg-primary/10 flex items-center justify-center text-[11px] uppercase font-bold text-primary">
									{meta.uploader?.charAt(0) || "U"}
								</div>
								<a
									href={meta.uploaderUrl || "#"}
									target="_blank"
									rel="noreferrer"
									className="text-sm font-semibold text-muted-foreground hover:text-primary transition-colors truncate"
								>
									{meta.uploader || "Anonymous Streamer"}
								</a>
							</div>
						</div>

						<div className="grid grid-cols-2 gap-4 rounded-xl bg-muted/30 p-4 text-sm border border-border/40">
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-[11px] text-muted-foreground font-bold uppercase tracking-wider">
									<Eye className="h-3.5 w-3.5 text-muted-foreground" /> View Count
								</span>
								<span className="font-mono text-base font-bold">
									{meta.viewCount?.toLocaleString() || "N/A"}
								</span>
							</div>
							<div className="space-y-1">
								<span className="flex items-center gap-1.5 text-[11px] text-muted-foreground font-bold uppercase tracking-wider">
									<ThumbsUp className="h-3.5 w-3.5 text-muted-foreground" /> Appraisals
								</span>
								<span className="font-mono text-base font-bold">
									{meta.likeCount?.toLocaleString() || "N/A"}
								</span>
							</div>
						</div>
					</div>
				</div>

				{/* INTERACTIVE CHAPTER MARKERS */}
				{meta.chapters && meta.chapters.length > 0 && (
					<div className="rounded-2xl border border-border/80 bg-card/40 p-5 space-y-4 shadow-lg backdrop-blur-md">
						<div className="flex items-center gap-2 border-b border-border/60 pb-2.5">
							<Bookmark className="h-4 w-4 text-primary" />
							<h3 className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
								Indexed Chapters ({meta.chapters.length})
							</h3>
						</div>
						<div className="max-h-[260px] overflow-y-auto space-y-2 pr-1 custom-scrollbar">
							{meta.chapters.map((ch, idx) => (
								<button
									key={idx}
									onClick={() => snapToTimeline(ch.startTime, ch.endTime)}
									className="w-full flex items-center justify-between p-2.5 text-left text-xs font-medium rounded-lg bg-muted/20 border border-transparent hover:border-primary/30 hover:bg-muted/50 transition-all group"
								>
									<span className="font-semibold truncate max-w-[70%] group-hover:text-primary transition-colors">
										{ch.title}
									</span>
									<span className="font-mono text-[10px] text-muted-foreground bg-background/60 border px-2 py-0.5 rounded">
										{formatTime(ch.startTime)}
									</span>
								</button>
							))}
						</div>
					</div>
				)}
			</div>

			{/* RIGHT COLUMN: Pipeline Configuration Options */}
			<div className="xl:col-span-7 flex flex-col space-y-6 pb-16">
				{/* INGESTION OPERATION MODE MODE SELECTOR */}
				<div className="rounded-xl border border-border/80 bg-card p-1.5 shadow-md flex items-center relative overflow-hidden">
					<button
						onClick={() => setDownloadMode("vod")}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-xs font-bold uppercase tracking-wider transition-all duration-200 ${
							downloadMode === "vod"
								? "bg-primary text-primary-foreground shadow-sm"
								: "text-muted-foreground hover:text-foreground hover:bg-muted/40"
						}`}
					>
						<Video className="h-4 w-4" /> High-Fidelity VOD Stream
					</button>
					<button
						onClick={() => setDownloadMode("chat")}
						disabled={!isChatSupported}
						className={`flex-1 flex items-center justify-center gap-2 rounded-lg py-3 text-xs font-bold uppercase tracking-wider transition-all duration-200 ${
							!isChatSupported
								? "opacity-40 cursor-not-allowed"
								: downloadMode === "chat"
									? "bg-primary text-primary-foreground shadow-sm"
									: "text-muted-foreground hover:text-foreground hover:bg-muted/40"
						}`}
					>
						<MessageSquare className="h-4 w-4" /> Live Chat Replay Log
					</button>
				</div>

				{/* TIMELINE CLIPIPING BOUNDARIES */}
				<div className="space-y-5 rounded-2xl border border-border bg-card p-6 shadow-sm transition-all hover:border-primary/10">
					<div className="flex items-center gap-2 border-b border-border/60 pb-3">
						<Scissors className="h-4 w-4 text-primary" />
						<h3 className="text-xs font-bold tracking-wider uppercase text-foreground">
							Timeline Boundary Extraction
						</h3>
					</div>

					<div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-1">
						<div className="space-y-3 p-4 rounded-xl bg-muted/20 border border-border/40 hover:border-primary/30 transition-colors group">
							<div className="flex justify-between items-center">
								<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider group-hover:text-primary transition-colors">
									In-Point Boundary
								</label>
								<span className="text-[10px] font-mono font-bold text-primary bg-primary/10 px-2 py-0.5 rounded-md">
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
									className="w-full h-1 bg-border rounded-lg appearance-none cursor-grab active:cursor-grabbing accent-primary"
								/>
							)}
							<input
								type="text"
								value={startTimeText}
								onChange={(e) => setStartTimeText(e.target.value)}
								onBlur={handleStartTextBlur}
								placeholder="00:00:00"
								className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs font-mono transition-all focus:ring-2 focus:ring-primary/40 focus:outline-none"
							/>
						</div>

						<div className="space-y-3 p-4 rounded-xl bg-muted/20 border border-border/40 hover:border-primary/30 transition-colors group">
							<div className="flex justify-between items-center">
								<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider group-hover:text-primary transition-colors">
									Out-Point Boundary
								</label>
								<span className="text-[10px] font-mono font-bold text-primary bg-primary/10 px-2 py-0.5 rounded-md">
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
									className="w-full h-1 bg-border rounded-lg appearance-none cursor-grab active:cursor-grabbing accent-primary"
								/>
							)}
							<input
								type="text"
								value={endTimeText}
								onChange={(e) => setEndTimeText(e.target.value)}
								onBlur={handleEndTextBlur}
								placeholder="00:00:00"
								className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs font-mono transition-all focus:ring-2 focus:ring-primary/40 focus:outline-none"
							/>
						</div>
					</div>
				</div>

				{/* STREAM QUALITY & ENCODING TOPOLOGY CONTROLS */}
				{downloadMode === "vod" && (
					<motion.div
						initial={{ opacity: 0, height: 0 }}
						animate={{ opacity: 1, height: "auto" }}
						className="space-y-5 rounded-2xl border border-border bg-card p-6 shadow-sm transition-all hover:border-primary/10"
					>
						<div className="flex items-center justify-between border-b border-border/60 pb-3">
							<div className="flex items-center gap-2">
								<Layers className="h-4 w-4 text-primary" />
								<h3 className="text-xs font-bold tracking-wider uppercase text-foreground">
									Format Topology & Streams
								</h3>
							</div>

							<div className="flex rounded-md border p-0.5 bg-muted/30 text-[10px] font-bold uppercase">
								<button
									onClick={() => setTrackSelectionMode("unified")}
									className={`px-2 py-1 rounded-sm ${trackSelectionMode === "unified" ? "bg-background shadow-sm text-primary" : "text-muted-foreground"}`}
								>
									Unified
								</button>
								<button
									onClick={() => setTrackSelectionMode("split")}
									className={`px-2 py-1 rounded-sm ${trackSelectionMode === "split" ? "bg-background shadow-sm text-primary" : "text-muted-foreground"}`}
								>
									Split Tracks
								</button>
							</div>
						</div>

						<div className="grid grid-cols-1 md:grid-cols-2 gap-5 pt-1">
							{trackSelectionMode === "unified" ? (
								<div className="space-y-2 md:col-span-2">
									<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider">
										Target Source Matrix Profile
									</label>
									<select
										value={selectedFormatId}
										onChange={(e) => setSelectedFormatId(e.target.value)}
										className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs transition-all focus:ring-2 focus:ring-primary/40 focus:outline-none cursor-pointer"
									>
										<option value="best">Highest Efficiency (Auto Aggregate)</option>
										<option value="worst">Lowest Profile Matrix</option>
										{preMergedFormats.length > 0 && (
											<optgroup label="Pre-Merged Stream Layers (Multiplexed)">
												{preMergedFormats.map((f) => (
													<option key={f.formatId} value={f.formatId}>{f.uiLabel}</option>
												))}
											</optgroup>
										)}
										{videoOnlyFormats.length > 0 && (
											<optgroup label="Video Intermediaries (Requires Audio Cross-Merge)">
												{videoOnlyFormats.map((f) => (
													<option key={f.formatId} value={f.formatId}>{f.uiLabel}</option>
												))}
											</optgroup>
										)}
										{audioOnlyFormats.length > 0 && (
											<optgroup label="Isolated Audio Layers">
												{audioOnlyFormats.map((f) => (
													<option key={f.formatId} value={f.formatId}>{f.uiLabel}</option>
												))}
											</optgroup>
										)}
									</select>
								</div>
							) : (
								<>
									<div className="space-y-2">
										<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider">
											Explicit Video Track ID
										</label>
										<select
											value={videoFormatId}
											onChange={(e) => setVideoFormatId(e.target.value)}
											className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs transition-all focus:ring-2 focus:ring-primary/40"
										>
											<option value="">Best Quality Video Track</option>
											{videoOnlyFormats.concat(preMergedFormats).map((f) => (
												<option key={f.formatId} value={f.formatId}>{f.uiLabel}</option>
											))}
										</select>
									</div>
									<div className="space-y-2">
										<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider">
											Explicit Audio Track ID
										</label>
										<select
											value={audioFormatId}
											onChange={(e) => setAudioFormatId(e.target.value)}
											className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs transition-all focus:ring-2 focus:ring-primary/40"
										>
											<option value="">Best Quality Audio Track</option>
											{audioOnlyFormats.concat(preMergedFormats).map((f) => (
												<option key={f.formatId} value={f.formatId}>{f.uiLabel}</option>
											))}
										</select>
									</div>
								</>
							)}

							<div className="space-y-3 p-4 rounded-xl bg-muted/10 border border-border/40 md:col-span-2 grid grid-cols-1 sm:grid-cols-2 gap-4 items-center">
								<div className="flex items-center gap-3">
									<input
										type="checkbox"
										id="audioOnlyCheck"
										checked={audioOnly}
										onChange={(e) => setAudioOnly(e.target.checked)}
										className="rounded border-input bg-background text-primary focus:ring-primary h-4 w-4 accent-primary cursor-pointer"
									/>
									<label htmlFor="audioOnlyCheck" className="text-xs font-bold text-foreground uppercase tracking-wider cursor-pointer flex items-center gap-1.5">
										<Volume2 className="h-3.5 w-3.5 text-primary" /> Strip Video (Audio Only)
									</label>
								</div>

								<div className="space-y-1">
									{audioOnly ? (
										<>
											<label className="text-[10px] font-bold text-muted-foreground uppercase tracking-wider">Audio Output Standard</label>
											<select
												value={audioFormat}
												onChange={(e) => setAudioFormat(e.target.value as AudioFormat)}
												className="w-full rounded-lg border border-input bg-background px-2 py-1 text-xs transition-all"
											>
												<option value="best">Lossless Best Target</option>
												<option value="mp3">MPEG Audio Layer 3 (.mp3)</option>
												<option value="m4a">MPEG-4 Audio (.m4a)</option>
												<option value="flac">Free Lossless Codec (.flac)</option>
												<option value="wav">Waveform Audio (.wav)</option>
											</select>
										</>
									) : (
										<>
											<label className="text-[10px] font-bold text-muted-foreground uppercase tracking-wider">Container Output Multiplex</label>
											<select
												value={selectedOutputContainer}
												onChange={(e) => setSelectedOutputContainer(e.target.value as VideoFormat)}
												className="w-full rounded-lg border border-input bg-background px-2 py-1 text-xs transition-all"
											>
												<option value="any">Native Ingestion Stream (No Remux)</option>
												<option value="mp4">MPEG-4 Wrapper (.mp4)</option>
												<option value="mkv">Matroska Container (.mkv)</option>
												<option value="webm">WebM Layer (.webm)</option>
											</select>
										</>
									)}
								</div>
							</div>
						</div>
					</motion.div>
				)}

				{/* FILE SYSTEM TARGET DESTINATION */}
				<div className="space-y-5 rounded-2xl border border-border bg-card p-6 shadow-sm transition-all hover:border-primary/10">
					<div className="flex items-center gap-2 border-b border-border/60 pb-3">
						<HardDrive className="h-4 w-4 text-primary" />
						<h3 className="text-xs font-bold tracking-wider uppercase text-foreground">
							Target Destination System Path
						</h3>
					</div>
					<div className="space-y-4 pt-1">
						<div className="space-y-2">
							<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider">
								Save Pipeline Workspace
							</label>
							<div className="flex items-center gap-3">
								<div className="flex-1 rounded-lg border border-input bg-muted/20 px-3 py-2 text-xs text-muted-foreground font-mono shadow-inner truncate border-dashed">
									{saveFolder || "Default Application Environment Downloads"}
								</div>
								<button
									onClick={handleSelectFolder}
									className="flex items-center gap-2 rounded-lg bg-secondary px-4 py-2 text-xs font-bold uppercase tracking-wide text-secondary-foreground transition-all hover:bg-secondary/80 shrink-0 border"
								>
									<FolderOpen className="h-3.5 w-3.5" />
									Browse Target
								</button>
							</div>
						</div>
						<div className="space-y-2">
							<label className="text-[11px] font-bold text-muted-foreground uppercase tracking-wider">
								Overriding Filename Pattern (Optional)
							</label>
							<input
								type="text"
								value={fileName}
								onChange={(e) => setFileName(e.target.value)}
								placeholder="Default Engine Token Generation Pattern"
								className="w-full rounded-lg border border-input bg-background px-3 py-2 text-xs transition-all focus:ring-2 focus:ring-primary/40"
							/>
						</div>
					</div>
				</div>

				{/* POST PROCESSING INTERNALS & INTEGRATION CRADLE */}
				{downloadMode === "vod" && (
					<div className="space-y-4 rounded-2xl border border-border bg-card p-6 shadow-sm transition-all hover:border-primary/10">
						<div className="flex items-center gap-2 border-b border-border/60 pb-3">
							<Sparkles className="h-4 w-4 text-primary" />
							<h3 className="text-xs font-bold tracking-wider uppercase text-foreground">
								Media Integration & Post-Processing
							</h3>
						</div>

						<div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-4 pt-1 text-xs font-medium">
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={embedMetadata} onChange={(e) => setEmbedMetadata(e.target.checked)} className="accent-primary h-4 w-4" />
								<span>Inject Meta Tags</span>
							</label>
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={embedThumbnail} onChange={(e) => setEmbedThumbnail(e.target.checked)} className="accent-primary h-4 w-4" />
								<span>Embed Artwork</span>
							</label>
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={embedChapters} onChange={(e) => setEmbedChapters(e.target.checked)} className="accent-primary h-4 w-4" />
								<span>Inject Chapters</span>
							</label>
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={embedSubs} onChange={(e) => setEmbedSubs(e.target.checked)} className="accent-primary h-4 w-4" />
								<span>Embed Hard Subs</span>
							</label>
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={writeAutoSubs} onChange={(e) => setWriteAutoSubs(e.target.checked)} className="accent-primary h-4 w-4" />
								<span>Generate AI Subs</span>
							</label>
							<label className="flex items-center gap-2.5 p-2.5 bg-muted/20 border border-border/50 rounded-xl cursor-pointer hover:border-primary/20">
								<input type="checkbox" checked={sponsorblock} onChange={(e) => setSponsorblock(e.target.checked)} className="accent-primary h-4 w-4" />
								<span className="text-red-400 font-semibold">SponsorBlock API</span>
							</label>
						</div>
					</div>
				)}

				{/* ENGINE RUNTIME FLAGS ACCORDION */}
				<div className="rounded-2xl border border-border bg-card overflow-hidden shadow-sm">
					<button
						onClick={() => setShowAdvanced(!showAdvanced)}
						className="flex w-full items-center justify-between bg-muted/30 px-6 py-4 transition-colors hover:bg-muted/40"
					>
						<span className="flex items-center gap-2 text-xs font-bold uppercase tracking-wider text-foreground">
							<Settings2 className="h-4 w-4 text-primary" />
							Engine & Core Network Overrides
						</span>
						<span className="text-[10px] font-bold uppercase border bg-background px-2.5 py-1 rounded-md shadow-sm">
							{showAdvanced ? "Collapse Directives" : "Expose Directives"}
						</span>
					</button>
					<AnimatePresence>
						{showAdvanced && (
							<motion.div
								initial={{ opacity: 0, height: 0 }}
								animate={{ opacity: 1, height: "auto" }}
								exit={{ opacity: 0, height: 0 }}
								className="p-6 border-t border-border bg-muted/10 grid grid-cols-1 md:grid-cols-2 gap-5 text-xs"
							>
								{downloadMode === "vod" ? (
									<>
										<div className="space-y-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider">Parallel Worker Threads</label>
											<input type="number" value={threads} onChange={(e) => setThreads(Number(e.target.value))} min={1} max={64} className="w-full rounded-lg border border-input bg-background px-3 py-2" />
										</div>
										<div className="space-y-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider">Network Bandwidth Cap (e.g. 50K, 10M)</label>
											<input type="text" value={limitRate} onChange={(e) => setLimitRate(e.target.value)} placeholder="Unlimited Pipeline Pipe" className="w-full rounded-lg border border-input bg-background px-3 py-2 font-mono" />
										</div>
										<div className="space-y-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider flex items-center gap-1.5"><Globe className="h-3 w-3" /> Pass Authenticated Browser Session Cookies</label>
											<select value={cookiesBrowser} onChange={(e) => setCookiesBrowser(e.target.value)} className="w-full rounded-lg border border-input bg-background px-3 py-2">
												<option value="">Do Not Share App Cookies</option>
												<option value="chrome">Google Chrome Session</option>
												<option value="firefox">Mozilla Firefox Session</option>
												<option value="edge">Microsoft Edge Session</option>
												<option value="safari">Apple Safari Session</option>
											</select>
										</div>
										<div className="flex flex-col justify-end gap-2 p-1">
											<label className="flex items-center gap-2 cursor-pointer font-semibold">
												<input type="checkbox" checked={forceKeyframes} onChange={(e) => setForceKeyframes(e.target.checked)} className="accent-primary h-4 w-4" />
												<span>Force Absolute Keyframe Splits</span>
											</label>
											<label className="flex items-center gap-2 cursor-pointer font-semibold">
												<input type="checkbox" checked={liveFromStart} onChange={(e) => setLiveFromStart(e.target.checked)} className="accent-primary h-4 w-4" />
												<span>Ingest Live Streams From Genesis Point</span>
											</label>
										</div>
									</>
								) : (
									<>
										<div className="space-y-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider">Max Network Retries</label>
											<input type="number" value={maxRetries} onChange={(e) => setMaxRetries(Number(e.target.value))} min={1} className="w-full rounded-lg border border-input bg-background px-3 py-2" />
										</div>
										<div className="space-y-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider">Kick Architecture Concurrency Limit</label>
											<input type="number" value={kickConcurrency} onChange={(e) => setKickConcurrency(Number(e.target.value))} min={1} className="w-full rounded-lg border border-input bg-background px-3 py-2" />
										</div>
										<div className="space-y-2 md:col-span-2">
											<label className="font-bold text-muted-foreground uppercase tracking-wider">Empty Cycle Recovery Boundary Threshold</label>
											<input type="number" value={emptyCycleThreshold} onChange={(e) => setEmptyCycleThreshold(Number(e.target.value))} min={1} className="w-full rounded-lg border border-input bg-background px-3 py-2" />
										</div>
									</>
								)}
							</motion.div>
						)}
					</AnimatePresence>
				</div>

				{/* INGESTION PIPELINE EMISSION ACTUATOR */}
				<div className="pt-2">
					<button
						onClick={handleQueueDownload}
						className="group flex w-full items-center justify-center gap-3 rounded-2xl bg-primary px-4 py-4 text-xs font-bold uppercase tracking-widest text-primary-foreground shadow-lg shadow-primary/20 transition-all hover:bg-primary/95 focus-visible:ring-2 focus-visible:ring-primary active:scale-[0.99]"
					>
						<Download className="h-4 w-4 transition-transform group-hover:-translate-y-0.5" />
						Incept Job Matrix Execution
					</button>
				</div>
			</div>
		</motion.div>
	);
}