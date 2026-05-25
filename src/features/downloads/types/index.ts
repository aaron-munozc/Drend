import { z } from 'zod';

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------
export const PlatformSchema = z.enum(['twitch', 'kick']);
export type Platform = z.infer<typeof PlatformSchema>;

// ---------------------------------------------------------------------------
// MediaType
// ---------------------------------------------------------------------------
export const MediaTypeSchema = z.enum(['live', 'vod', 'clip']);
export type MediaType = z.infer<typeof MediaTypeSchema>;

// ---------------------------------------------------------------------------
// TaskStatus — mirrors Rust: Queued | Processing | Completed | Failed(String)
// The Rust #[serde(tag="kind", content="message")] on AppError is NOT used
// here; the DownloadManager uses a simpler inline representation via
// #[serde(rename_all = "camelCase")] so status arrives as the variant name
// (string) or { failed: string }.
// ---------------------------------------------------------------------------
export const TaskStatusSchema = z.union([
  z.enum(['queued', 'processing', 'completed']),
  z.object({ failed: z.string() }),
]);
export type TaskStatus = z.infer<typeof TaskStatusSchema>;

// ---------------------------------------------------------------------------
// DownloadTask — mirrors Rust DownloadTask struct
// ---------------------------------------------------------------------------
export const DownloadTaskSchema = z.object({
  taskId: z.string(),
  title: z.string(),
  progress: z.number().min(0).max(100),
  currentChunk: z.number().int().nonnegative(),
  totalEstimatedChunks: z.number().int().nonnegative().nullable(),
  status: TaskStatusSchema,
});
export type DownloadTask = z.infer<typeof DownloadTaskSchema>;

// ---------------------------------------------------------------------------
// ChatMetadata — what we send TO the backend
// ---------------------------------------------------------------------------
export const ChatMetadataSchema = z.object({
  chatId: z.string().min(1, 'VOD / Chat ID is required'),
  channelSlug: z.string().min(1, 'Channel slug is required'),
  platform: PlatformSchema,
  startTime: z.string().datetime({ offset: true }).nullable(),
  durationMs: z.number().int().nonnegative(),
});
export type ChatMetadata = z.infer<typeof ChatMetadataSchema>;

// ---------------------------------------------------------------------------
// StreamQuality — mirrors Rust StreamQuality struct
// ---------------------------------------------------------------------------
export const StreamQualitySchema = z.object({
  index: z.number().int().nonnegative(),
  label: z.string(),
  downloadUrl: z.string().url(),
});
export type StreamQuality = z.infer<typeof StreamQualitySchema>;

// ---------------------------------------------------------------------------
// UnifiedMetadata — returned by analyze_stream_url from the backend
// ---------------------------------------------------------------------------
export const UnifiedMetadataSchema = z.object({
  platform: PlatformSchema,
  mediaType: MediaTypeSchema,
  id: z.string(),
  title: z.string(),
  username: z.string(),
  thumbnailUrl: z.string().url().nullable(),
  durationMs: z.number().int().nonnegative(),
  qualities: z.array(StreamQualitySchema),
  chatInfo: ChatMetadataSchema.nullable(),
});
export type UnifiedMetadata = z.infer<typeof UnifiedMetadataSchema>;
