import { createFileRoute } from '@tanstack/react-router';
import { DownloadManager } from '../features/downloads/components/DownloadManager';

export const Route = createFileRoute('/')({
  component: IndexPage,
});

function IndexPage() {
  return (
    <div className="min-h-screen bg-zinc-950 text-white">
      {/* Top bar */}
      <header className="border-b border-zinc-800/60 px-6 py-4 flex items-center gap-3">
        <div className="flex items-center gap-2">
          <span className="inline-flex h-7 w-7 items-center justify-center rounded-lg bg-indigo-600">
            <svg className="h-4 w-4 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M3 16.5v2.25A2.25 2.25 0 0 0 5.25 21h13.5A2.25 2.25 0 0 0 21 18.75V16.5M16.5 12 12 16.5m0 0L7.5 12m4.5 4.5V3" />
            </svg>
          </span>
          <span className="text-sm font-semibold text-white tracking-tight">Stream Downloader</span>
        </div>
        <span className="ml-2 rounded-full border border-zinc-700 bg-zinc-800 px-2 py-0.5 text-xs text-zinc-400">
          dev
        </span>
      </header>

      {/* Page content */}
      <main className="mx-auto max-w-6xl px-6 py-8">
        <div className="mb-7">
          <h1 className="text-xl font-bold text-white tracking-tight">Chat Downloads</h1>
          <p className="text-sm text-zinc-500 mt-1">
            Queue Twitch VODs or Kick streams — progress streams live from the backend.
          </p>
        </div>

        <DownloadManager />
      </main>
    </div>
  );
}
