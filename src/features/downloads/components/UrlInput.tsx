import { motion } from "framer-motion";
import { Search } from "lucide-react";
import { useState } from "react";
import { useDownloader } from "@/features/downloads/hooks/useDownlaoder.ts";

export function UrlInput({ tabId }: { tabId: string }) {
	const [url, setUrl] = useState("");
	const { analyzeUrl } = useDownloader();

	const handleSubmit = (e: React.FormEvent) => {
		e.preventDefault();
		if (url.trim()) analyzeUrl(tabId, url);
	};

	return (
		<motion.div
			initial={{ opacity: 0, y: 10 }}
			animate={{ opacity: 1, y: 0 }}
			transition={{ duration: 0.3 }}
			className="flex h-full w-full items-center justify-center p-6"
		>
			<div className="w-full max-w-xl space-y-6 text-center">
				<div className="space-y-2">
					<h1 className="text-3xl font-semibold tracking-tight text-foreground">
						Download Stream
					</h1>
					<p className="text-muted-foreground">
						Paste a Twitch or Kick URL to analyze metadata and configure
						options.
					</p>
				</div>

				<form onSubmit={handleSubmit} className="flex gap-3">
					<div className="relative flex-1">
						<Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
						<input
							type="url"
							value={url}
							onChange={(e) => setUrl(e.target.value)}
							placeholder="https://twitch.tv/..."
							className="w-full rounded-md border border-input bg-background px-9 py-2 text-sm shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
							required
						/>
					</div>
					<button
						type="submit"
						className="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground shadow transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
					>
						Analyze
					</button>
				</form>
			</div>
		</motion.div>
	);
}
