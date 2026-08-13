// ─────────────────────────────────────────────────────────────────────────────
// Primitive value types
// ─────────────────────────────────────────────────────────────────────────────

export interface ObjectColor {
	alpha: number; // 0–255
	red: number;
	green: number;
	blue: number;
}

// ─────────────────────────────────────────────────────────────────────────────
// Overlay layer types (new pipeline extension)
// ─────────────────────────────────────────────────────────────────────────────

/** Solid-color rounded rectangle drawn above the background, below chat. */
export interface CustomShapeOverlay {
	x: number;
	y: number;
	width: number;
	height: number;
	color: ObjectColor;
	cornerRadius: number;
}

/** Image asset composited above the background, below chat. */
export interface CustomImageOverlay {
	/** Absolute path to a PNG / JPEG / WEBP / GIF on disk. */
	assetPath: string;
	x: number;
	y: number;
	/** Drawn at native width when omitted. */
	width?: number;
	/** Drawn at native height when omitted. */
	height?: number;
	/** 0.0–1.0 */
	alpha: number;
}

// ─────────────────────────────────────────────────────────────────────────────
// Discriminated union / enum mirrors
// ─────────────────────────────────────────────────────────────────────────────

export type BackgroundMode =
	| "transparent"
	| "lumaMatte"
	| "chromaKeyGreen"
	| "customColor";

export type EvictionStrategy = "timed" | "pushOnly";

export type QualityPreset = "draft" | "standard" | "high";

export type TimelineMismatchStrategy =
	| "freezeLastFrame"
	| "renderClearCanvas"
	| "loopChatLog";

// ─────────────────────────────────────────────────────────────────────────────
// Main render configuration — mirrors args.rs RenderVideoArgs exactly
// ─────────────────────────────────────────────────────────────────────────────

export interface RenderVideoArgs {
	// ── Output ──────────────────────────────────────────────────────────────
	outputPath: string;

	// ── Canvas ──────────────────────────────────────────────────────────────
	width: number;
	height: number;
	fps: number;

	// ── Background ──────────────────────────────────────────────────────────
	backgroundMode: BackgroundMode;
	backgroundColor: ObjectColor;

	// ── Typography ───────────────────────────────────────────────────────────
	fontName: string;
	fontSize: number;
	lineSpacing: number;

	// ── Layout ───────────────────────────────────────────────────────────────
	messageSpacing: number;
	padding: number;

	// ── Text style ───────────────────────────────────────────────────────────
	messageColor: ObjectColor;
	outlineUsernames: boolean;
	usernameOutlineWidth?: number;
	usernameShadow: boolean;

	// ── Bubble ───────────────────────────────────────────────────────────────
	bubbleModeFullWidth: boolean;
	bubbleColor: ObjectColor;
	bubbleRadius: number;
	bubblePadding: number;

	// ── Animations ───────────────────────────────────────────────────────────
	animSlide: boolean;
	animFadeIn: boolean;

	// ── Lifecycle ────────────────────────────────────────────────────────────
	evictionStrategy: EvictionStrategy;
	messageHoldSeconds: number;
	messageFadeOutSeconds: number;

	// ── Quality ──────────────────────────────────────────────────────────────
	qualityPreset: QualityPreset;
	maxCachedEmotes: number;
	centerEmotesVertically: boolean;
	createPremultipliedAlphaEmotes: boolean;

	// ── Users ────────────────────────────────────────────────────────────────
	pinnedUsers: string[];
	highlightColor: ObjectColor;
	pinDurationSecs: number;
	skipUsers: string[];

	// ── Grouping ─────────────────────────────────────────────────────────────
	groupMessages: boolean;
	groupMessagesWindowSecs: number;

	// ── Time window ──────────────────────────────────────────────────────────
	startMs?: number;
	endMs?: number;
	timeZeroMs?: number;

	// ── Live video overlay (base video composited with chat) ─────────────────
	overlayVideoPath?: string;
	overlayX?: number;
	overlayY?: number;
	overlayWidth?: number;
	overlayHeight?: number;

	// ── Pipeline extension fields ─────────────────────────────────────────────
	useImmediatePipeOverlay: boolean;
	shapeOverlays: CustomShapeOverlay[];
	imageOverlays: CustomImageOverlay[];
	timelineMismatchStrategy: TimelineMismatchStrategy;
}

/** Canonical defaults strictly mirroring args.rs struct defaults */
export const RENDER_DEFAULTS: RenderVideoArgs = {
	outputPath: "",
	width: 400,
	height: 800,
	fps: 24,
	backgroundMode: "lumaMatte",
	backgroundColor: { alpha: 255, red: 20, green: 20, blue: 20 },
	fontName: "Inter",
	fontSize: 20,
	lineSpacing: 6,
	messageSpacing: 12,
	padding: 20,
	messageColor: { alpha: 255, red: 240, green: 240, blue: 240 },
	outlineUsernames: false,
	usernameOutlineWidth: undefined,
	usernameShadow: false,
	bubbleModeFullWidth: false,
	bubbleColor: { alpha: 255, red: 0, green: 0, blue: 0 },
	bubbleRadius: 8,
	bubblePadding: 8,
	animSlide: false,
	animFadeIn: false,
	evictionStrategy: "pushOnly",
	messageHoldSeconds: 5,
	messageFadeOutSeconds: 2,
	qualityPreset: "standard",
	maxCachedEmotes: 180,
	centerEmotesVertically: true,
	createPremultipliedAlphaEmotes: true,
	pinnedUsers: [],
	highlightColor: { alpha: 255, red: 255, green: 215, blue: 0 },
	pinDurationSecs: 10,
	skipUsers: ["BotRix", "KickBot"],
	groupMessages: false,
	groupMessagesWindowSecs: 0,
	startMs: undefined,
	endMs: undefined,
	timeZeroMs: undefined,
	overlayVideoPath: undefined,
	overlayX: 0,
	overlayY: 0,
	overlayWidth: undefined,
	overlayHeight: undefined,
	useImmediatePipeOverlay: false,
	shapeOverlays: [],
	imageOverlays: [],
	timelineMismatchStrategy: "freezeLastFrame",
};

// ─────────────────────────────────────────────────────────────────────────────
// Download / VOD / chat types (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

export interface NormalizedFormat {
	formatId: string;
	resolutionLabel: string;
	fps: number | null;
	extension: string;
	hasVideo: boolean;
	hasAudio: boolean;
	sizeBytes: number | null;
	bitrate: number | null;
	uiLabel: string;
	url: string;
}

export interface Chapter {
	startTime: number;
	endTime: number;
	title: string;
}

export interface Metadata {
	id: string;
	title: string;
	description?: string;
	duration?: number;

	uploader?: string;
	uploaderId?: string;
	uploaderUrl?: string;
	thumbnail?: string;
	viewCount?: number;
	likeCount?: number;
	commentCount?: number;

	timestamp?: number;
	uploadDate?: string;

	isLive: boolean;
	wasLive: boolean;
	isUpcoming: boolean;
	ageLimit: number;

	availability?: string;
	tags: string[];
	categories: string[];
	chapters: Chapter[];
	availableSubs: string[];

	formats: NormalizedFormat[];

	extractor?: string;
	isChatSupported: boolean;
	originalUrl: string;
	webpageUrl?: string;
}

export interface FrontendVodOptions {
	saveFolder?: string;
	fileName?: string;
	videoFormatId?: string;
	audioFormatId?: string;
	audioOnly?: boolean;
	startMs?: number;
	endMs?: number;
	threads?: number;
	limitRate?: string;
}

export interface FrontendChatOptions {
	startMs?: number;
	endMs?: number;
	maxRetries?: number;
	kickConcurrency?: number;
	saveFolder?: string;
	fileName?: string;
}

// ─────────────────────────────────────────────────────────────────────────────
// Task types
// ─────────────────────────────────────────────────────────────────────────────

export type TaskStatus =
	| "queued"
	| "processing"
	| "merging"
	| "completed"
	| "cancelled"
	| { failed: string };

export type TaskType = "chatDownload" | "vodDownload" | "chatRender";

export interface AppTask {
	taskId: string;
	taskType: TaskType;
	title: string;
	progress: number;
	status: TaskStatus;
	statusText: string | null;
}