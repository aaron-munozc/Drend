export type Platform = "twitch" | "kick";
export type StreamStatus = "live" | "clip" | "vod" | "offline";
export type VideoFormat = "mp4" | "mkv" | "ts";

// Matches the library QualityPreference enum serialized across Tauri
export type QualityPreference =
	| "best"
	| "worst"
	| { index: number }
	| { height: number };

export interface StreamResolution {
	width: number;
	height: number;
}

export interface StreamQuality {
	index: number;
	uri: string;
	resolution?: StreamResolution;
	bandwidth?: number;
}

export interface StreamMetadata {
	chatId?: number;
	startTime?: string;
	duration?: number;
	title?: string;
	thumbnailUrl?: string;
	viewerCount?: number;
	views?: number;
	username?: string;
	followers?: number;
	source?: string;
	playbackUrl?: string;
	vodUuid?: string;
	streamStatus?: StreamStatus;
	platform: Platform;
}

// The new wrapper struct returned by analyze_stream_url
export interface Metadata {
	streamMetadata: StreamMetadata;
	qualities: StreamQuality[];
}

export interface FrontendVodOptions {
	quality?: QualityPreference;
	format?: VideoFormat;
	threads?: number;
	startMs?: number;
	endMs?: number;
	bufferMs?: number;
	saveFolder?: string;
	fileName?: string;
}

export interface FrontendChatOptions {
	startMs?: number;
	endMs?: number;
	bufferMs?: number;
	maxRetries?: number;
	kickConcurrency?: number;
	emptyCycleThreshold?: number;
	saveFolder?: string;
	fileName?: string;
}
