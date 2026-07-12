export interface Color {
	alpha: number;
	red: number;
	green: number;
	blue: number;
}

export type BackgroundMode =
	| "transparent"
	| "lumaMatte"
	| "chromaKeyGreen"
	| "customColor";
export type EvictionStrategy = "timed" | "pushOnly";
export type QualityPreset = "draft" | "standard" | "high"; // Added to match Rust enum

export interface RenderVideoArgs {
	outputPath: string;
	width: number;
	height: number;
	fps: number;
	backgroundMode: BackgroundMode;
	backgroundColor: Color;
	fontName: string;
	fontSize: number;
	lineSpacing: number;
	messageSpacing: number;
	messageColor: Color;
	padding: number;
	outlineUsernames: boolean;
	usernameOutlineWidth: number | null;
	usernameShadow: boolean;
	bubbleModeFullWidth: boolean;
	bubbleColor: Color;
	bubbleRadius: number;
	bubblePadding: number;
	animSlide: boolean;
	animFadeIn: boolean;
	evictionStrategy: EvictionStrategy;
	qualityPreset: QualityPreset;
	maxCachedEmotes: number;
	messageHoldSeconds: number;
	messageFadeOutSeconds: number;
	pinnedUsers: string[];
	highlightColor: Color;
	pinDurationSecs: number;
	skipUsers: string[];
	startMs: number | null;
	endMs: number | null;
	timeZeroMs: number | null;
	groupMessages: boolean;
	groupMessagesWindowSecs: number;
	centerEmotesVertically: boolean;
	createPremultipliedAlphaEmotes: boolean;
}

export const defaultRenderArgs: RenderVideoArgs = {
	outputPath: "",
	width: 400,
	height: 800,
	fps: 30,
	backgroundMode: "chromaKeyGreen",
	backgroundColor: { alpha: 255, red: 20, green: 20, blue: 20 },
	fontName: "Inter",
	fontSize: 20.0,
	lineSpacing: 6,
	messageSpacing: 12,
	messageColor: { alpha: 255, red: 240, green: 240, blue: 240 },
	padding: 20,
	outlineUsernames: false,
	usernameOutlineWidth: null,
	usernameShadow: false,
	bubbleModeFullWidth: false,
	bubbleColor: { alpha: 255, red: 0, green: 0, blue: 0 },
	bubbleRadius: 8.0,
	bubblePadding: 8,
	animSlide: true,
	animFadeIn: false,
	evictionStrategy: "pushOnly",
	qualityPreset: "standard",
	maxCachedEmotes: 256,
	messageHoldSeconds: 5,
	messageFadeOutSeconds: 2,
	pinnedUsers: [],
	highlightColor: { alpha: 255, red: 255, green: 215, blue: 0 },
	pinDurationSecs: 10,
	skipUsers: ["BotRix", "KickBot"],
	startMs: null,
	endMs: null,
	timeZeroMs: null,
	groupMessages: false,
	groupMessagesWindowSecs: 0,
	centerEmotesVertically: true,
	createPremultipliedAlphaEmotes: true,
};
