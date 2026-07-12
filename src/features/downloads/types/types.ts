export type Platform = "twitch" | "kick";
export type StreamStatus = "live" | "clip" | "vod" | "offline";

export type VideoFormat = "any" | "mp4" | "mkv" | "webm";
export type AudioFormat = "best" | "mp3" | "m4a" | "flac" | "wav";

export interface NormalizedFormat {
	formatId: string;
	resolutionLabel: string;
	fps?: number;
	extension: string;
	hasVideo: boolean;
	hasAudio: boolean;
	sizeBytes?: number;
	bitrate?: number;
	uiLabel: string;
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

export interface Chapter {
	title: string;
	startTime: number;
	endTime: number;
}

export interface NormalizedMetadata {
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
	tags: string[];
	categories: string[];
	chapters: Chapter[];
	availableSubs: string[];
	formats: NormalizedFormat[];
	extractor?: string;
	isChatSupported: boolean;
	originalUrl: string;
}

export interface Metadata {
	normalized: NormalizedMetadata;
	streamMetadata?: StreamMetadata;
}

export interface FrontendVodOptions {
	saveFolder?: string;
	fileName?: string;
	videoFormatId?: string;
	audioFormatId?: string;
	resolution?: number;
	videoFormat?: VideoFormat;
	audioOnly?: boolean;
	audioFormat?: AudioFormat;
	startMs?: number;
	endMs?: number;
	forceKeyframes?: boolean;
	threads?: number;
	limitRate?: string;
	cookiesBrowser?: string;
	liveFromStart?: boolean;
	embedMetadata?: boolean;
	embedThumbnail?: boolean;
	embedChapters?: boolean;
	embedSubs?: boolean;
	writeAutoSubs?: boolean;
	subLangs?: string[];
	sponsorblock?: boolean;
}

export interface FrontendChatOptions {
	startMs?: number;
	endMs?: number;
	maxRetries?: number;
	kickConcurrency?: number;
	emptyCycleThreshold?: number;
	saveFolder?: string;
	fileName?: string;
}