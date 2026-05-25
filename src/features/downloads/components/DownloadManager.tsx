import { useDownloadManager } from '../hooks/useDownloadManager';
import { QueueForm } from './QueueForm';
import { TaskList } from './TaskList';

/**
 * Root feature component.  Mounts the Tauri event listener exactly once
 * (via the hook) and composes the form + list.
 */
export function DownloadManager() {
  useDownloadManager();

  return (
    <div className="grid grid-cols-1 lg:grid-cols-[420px_1fr] gap-6 items-start">
      <QueueForm />
      <TaskList />
    </div>
  );
}
