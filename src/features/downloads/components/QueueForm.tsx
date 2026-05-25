import { useState } from 'react';
import { useDownloadStore } from '../store/downloadStore';

const INPUT_CLASS =
  'w-full rounded-lg bg-zinc-900 border border-zinc-700 px-3 py-2 text-sm text-white ' +
  'placeholder-zinc-600 outline-none focus:border-indigo-500 focus:ring-1 focus:ring-indigo-500/50 transition';

function formatDuration(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);

  if (hours > 0) {
    return `${hours}h ${minutes % 60}m`;
  }
  if (minutes > 0) {
    return `${minutes}m ${seconds % 60}s`;
  }
  return `${seconds}s`;
}

export function QueueForm() {
  const analyzeUrl = useDownloadStore((s) => s.analyzeUrl);
  const queueDownload = useDownloadStore((s) => s.queueDownload);
  const analyzedMetadata = useDownloadStore((s) => s.analyzedMetadata);
  const isAnalyzing = useDownloadStore((s) => s.isAnalyzing);
  const error = useDownloadStore((s) => s.error);
  const clearError = useDownloadStore((s) => s.clearError);
  const clearAnalyzedMetadata = useDownloadStore((s) => s.clearAnalyzedMetadata);

  const [url, setUrl] = useState('');
  const [isQueuing, setIsQueuing] = useState(false);
  const [queued, setQueued] = useState(false);

  const handleAnalyze = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim()) return;

    clearError();
    try {
      await analyzeUrl(url.trim());
    } catch {
      // error already set in store
    }
  };

  const handleQueue = async () => {
    if (!analyzedMetadata?.chatInfo) return;

    setIsQueuing(true);
    try {
      await queueDownload(analyzedMetadata.chatInfo, analyzedMetadata.title);
      setQueued(true);
      setTimeout(() => {
        setQueued(false);
        clearAnalyzedMetadata();
        setUrl('');
      }, 2000);
    } catch {
      // error already set in store
    } finally {
      setIsQueuing(false);
    }
  };

  const handleReset = () => {
    clearAnalyzedMetadata();
    clearError();
    setUrl('');
  };

  return (
    <div className="rounded-2xl border border-zinc-800 bg-zinc-900/60 backdrop-blur p-6 space-y-5">
      <div>
        <h2 className="text-base font-semibold text-white">Queue Download</h2>
        <p className="text-xs text-zinc-500 mt-0.5">
          Paste a Twitch VOD/clip or Kick VOD/clip URL to download chat logs.
        </p>
      </div>

      {!analyzedMetadata ? (
        <>
          <form onSubmit={handleAnalyze} className="space-y-4">
            <div>
              <label className="text-xs font-medium text-zinc-400 uppercase tracking-wider mb-2 block">
                Stream URL
              </label>
              <input
                className={INPUT_CLASS}
                placeholder="https://www.twitch.tv/videos/123456789"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                disabled={isAnalyzing}
              />
            </div>

            {error && (
              <p className="text-xs text-red-400">{error}</p>
            )}

            <button
              type="submit"
              disabled={isAnalyzing || !url.trim()}
              className={`w-full rounded-lg px-4 py-2.5 text-sm font-semibold transition-all duration-200
                ${isAnalyzing
                  ? 'bg-indigo-700 text-indigo-300 cursor-wait opacity-70'
                  : 'bg-indigo-600 hover:bg-indigo-500 active:scale-[0.98] text-white'
                }`}
            >
              {isAnalyzing ? 'Analyzing…' : 'Analyze URL'}
            </button>
          </form>
        </>
      ) : (
        <div className="space-y-4">
          {/* Metadata Display */}
          <div className="rounded-lg bg-zinc-800/50 border border-zinc-700 p-4 space-y-3">
            {analyzedMetadata.thumbnailUrl && (
              <div className="aspect-video rounded-lg overflow-hidden bg-zinc-900">
                <img
                  src={analyzedMetadata.thumbnailUrl}
                  alt={analyzedMetadata.title}
                  className="w-full h-full object-cover"
                />
              </div>
            )}

            <div className="space-y-2">
              <h3 className="text-sm font-semibold text-white line-clamp-2">
                {analyzedMetadata.title}
              </h3>
              <div className="flex items-center gap-2 text-xs text-zinc-400">
                <span className="capitalize">{analyzedMetadata.platform}</span>
                <span>•</span>
                <span>{analyzedMetadata.username}</span>
                <span>•</span>
                <span>{formatDuration(analyzedMetadata.durationMs)}</span>
              </div>
            </div>

            {analyzedMetadata.chatInfo ? (
              <div className="pt-2 border-t border-zinc-700">
                <p className="text-xs text-zinc-500">
                  Ready to download chat logs from this stream.
                </p>
              </div>
            ) : (
              <div className="pt-2 border-t border-zinc-700">
                <p className="text-xs text-red-400">
                  Chat metadata not available for this stream.
                </p>
              </div>
            )}
          </div>

          {error && (
            <p className="text-xs text-red-400">{error}</p>
          )}

          <div className="flex gap-2">
            <button
              onClick={handleQueue}
              disabled={isQueuing || queued || !analyzedMetadata.chatInfo}
              className={`flex-1 rounded-lg px-4 py-2.5 text-sm font-semibold transition-all duration-200
                ${queued
                  ? 'bg-emerald-600 text-white cursor-default'
                  : isQueuing
                  ? 'bg-indigo-700 text-indigo-300 cursor-wait opacity-70'
                  : !analyzedMetadata.chatInfo
                  ? 'bg-zinc-700 text-zinc-500 cursor-not-allowed'
                  : 'bg-indigo-600 hover:bg-indigo-500 active:scale-[0.98] text-white'
                }`}
            >
              {queued ? '✓ Queued!' : isQueuing ? 'Queueing…' : 'Queue Download'}
            </button>
            <button
              onClick={handleReset}
              disabled={isQueuing || queued}
              className="rounded-lg px-4 py-2.5 text-sm font-medium border border-zinc-700 bg-zinc-800 text-zinc-400 hover:border-zinc-600 hover:text-zinc-300 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
