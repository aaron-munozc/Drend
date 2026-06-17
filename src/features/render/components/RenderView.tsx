import { invoke } from "@tauri-apps/api/core";
import { open, message } from "@tauri-apps/plugin-dialog";
import {
	Plus,
	X,
	FileJson,
	Settings,
	Play,
	Type,
	LayoutTemplate,
	Film,
	Clock,
	Users,
	Image as ImageIcon,
	MessageSquare,
	FolderOpen,
	FileCode2
} from "lucide-react";
import { useRenderTabStore, RenderTab } from "@/store/useRenderTabStore.ts";
import { ObjectColor } from "../types/types.ts";

const objToHex = (c: ObjectColor) =>
	"#" +
	[c.red, c.green, c.blue].map((x) => x.toString(16).padStart(2, "0")).join("");

const hexToObj = (hex: string, alpha: number = 255): ObjectColor => {
	const clean = hex.replace("#", "");
	return {
		alpha,
		red: parseInt(clean.substring(0, 2), 16) || 0,
		green: parseInt(clean.substring(2, 4), 16) || 0,
		blue: parseInt(clean.substring(4, 6), 16) || 0,
	};
};

export function RenderView() {
	const { tabs, activeTabId, addTab, closeTab, setActiveTab } =
		useRenderTabStore();

	if (!activeTabId && tabs.length > 0) setActiveTab(tabs[0].id);
	const activeTab = tabs.find((t) => t.id === activeTabId);

	return (
		<div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground font-sans selection:bg-primary/20">
			{/* MAC/VSCODE STYLE TAB BAR */}
			<div className="flex items-end gap-px bg-muted/40 px-3 pt-3 border-b border-border overflow-x-auto no-scrollbar">
				{tabs.map((tab) => (
					<button
						key={tab.id}
						onClick={() => setActiveTab(tab.id)}
						className={`group relative flex shrink-0 items-center gap-3 rounded-t-lg border border-b-0 px-4 py-2.5 text-sm font-medium transition-all duration-200 ease-out ${
							activeTabId === tab.id
								? "border-border bg-background text-foreground shadow-[0_-2px_10px_rgba(0,0,0,0.02)] z-10"
								: "border-transparent bg-transparent text-muted-foreground hover:bg-muted hover:text-foreground"
						}`}
					>
						{/* Active Tab Indicator Line */}
						{activeTabId === tab.id && (
							<div className="absolute top-0 left-0 right-0 h-0.5 bg-primary rounded-t-lg" />
						)}
						<FileCode2 className="h-4 w-4 opacity-70" />
						<span className="max-w-35 truncate">{tab.title}</span>
						<div
							role="button"
							tabIndex={0}
							onClick={(e) => {
								e.stopPropagation();
								closeTab(tab.id);
							}}
							className={`rounded-md p-1 transition-all ${
								activeTabId === tab.id
									? "text-muted-foreground hover:bg-muted hover:text-foreground"
									: "opacity-0 text-muted-foreground hover:bg-background group-hover:opacity-100"
							}`}
						>
							<X className="h-3.5 w-3.5" />
						</div>
					</button>
				))}
				<button
					onClick={addTab}
					className="ml-2 mb-1.5 shrink-0 rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
					title="New Render Configuration"
				>
					<Plus className="h-5 w-5" />
				</button>
			</div>

			<div className="flex-1 min-h-0 overflow-y-auto relative scroll-smooth">
				{activeTab && <RenderConfiguration tab={activeTab} />}
			</div>
		</div>
	);
}

// --- CONFIGURATION FORM COMPONENT ---

function RenderConfiguration({ tab }: { tab: RenderTab }) {
	const { updateTab, updateTabOptions } = useRenderTabStore();
	const opts = tab.options;

	const handleSelectInputFile = async () => {
		const selected = await open({
			multiple: false,
			filters: [{ name: "JSON Chat Log", extensions: ["jsonl"] }],
		});
		if (selected && typeof selected === "string") {
			const filename = selected.split(/[\\/]/).pop() || "Chat Render";
			updateTab(tab.id, { jsonFilePath: selected, title: filename });
		}
	};

	const handleSelectOutputDir = async () => {
		const selected = await open({ directory: true, multiple: false });
		if (selected && typeof selected === "string") {
			updateTabOptions(tab.id, { outputPath: selected });
		}
	};

	const handleQueueRender = async () => {
		if (!tab.jsonFilePath) {
			await message("Please select an input JSON file first.", {
				title: "Missing Input",
				kind: "error",
			});
			return;
		}
		try {
			await invoke("queue_chat_render", {
				jsonFilePath: tab.jsonFilePath,
				options: opts,
			});
			await message("Chat render added to processing queue.", {
				title: "Render Queued",
				kind: "info",
			});
		} catch (err: any) {
			await message(`Failed to queue render: ${err.toString()}`, {
				title: "Error",
				kind: "error",
			});
		}
	};

	if (!tab.jsonFilePath) {
		return (
			<div className="flex h-full w-full items-center justify-center p-8 bg-background/50">
				<div className="flex flex-col items-center max-w-lg w-full p-10 rounded-3xl border-2 border-dashed border-border bg-card/50 shadow-sm animate-in fade-in zoom-in-95 duration-500 ease-out">
					<div className="h-20 w-20 rounded-full bg-primary/10 flex items-center justify-center mb-6 shadow-inner">
						<FileJson className="w-10 h-10 text-primary" />
					</div>
					<h2 className="text-2xl font-bold tracking-tight mb-2">Select Chat Data</h2>
					<p className="text-muted-foreground text-center mb-8 leading-relaxed">
						Import a previously exported JSONL chat file to begin configuring your visual render layout and timeline settings.
					</p>
					<button
						onClick={handleSelectInputFile}
						className="inline-flex items-center gap-2 px-6 py-3 bg-primary text-primary-foreground text-sm font-semibold rounded-xl shadow-md shadow-primary/20 hover:bg-primary/90 hover:-translate-y-0.5 transition-all focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-background active:scale-95"
					>
						<FolderOpen className="w-4 h-4" />
						Browse JSON File
					</button>
				</div>
			</div>
		);
	}

	return (
		<div className="max-w-350 mx-auto p-6 md:p-8 space-y-8 pb-32 animate-in fade-in duration-300">
			{/* HEADER ACTIONS */}
			<div className="flex flex-col md:flex-row md:items-center justify-between gap-4 bg-card px-6 py-5 rounded-2xl border border-border shadow-sm">
				<div className="flex items-center gap-4 overflow-hidden">
					<div className="p-3 bg-primary/10 rounded-xl shrink-0">
						<FileCode2 className="w-6 h-6 text-primary" />
					</div>
					<div className="space-y-1 min-w-0">
						<h2 className="font-semibold text-foreground tracking-tight">Active Source File</h2>
						<p
							className="text-sm text-muted-foreground truncate font-mono bg-muted/50 px-2 py-0.5 rounded-md inline-block max-w-full"
							title={tab.jsonFilePath}
						>
							{tab.jsonFilePath}
						</p>
					</div>
				</div>
				<button
					onClick={handleSelectInputFile}
					className="shrink-0 inline-flex items-center gap-2 px-4 py-2 bg-secondary text-secondary-foreground text-sm font-medium rounded-lg hover:bg-secondary/80 transition-colors focus:outline-none focus:ring-2 focus:ring-ring"
				>
					Change Source
				</button>
			</div>

			{/* MASONRY/GRID LAYOUT */}
			<div className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 gap-6 items-start">

				{/* --- 1. VIDEO OUTPUT --- */}
				<Section title="Video Output" icon={<Film />} description="Resolution and background settings">
					<div className="grid grid-cols-2 gap-4">
						<Input
							label="Width (px)"
							type="number"
							value={opts.width}
							onChange={(v) => updateTabOptions(tab.id, { width: Number(v) })}
						/>
						<Input
							label="Height (px)"
							type="number"
							value={opts.height}
							onChange={(v) => updateTabOptions(tab.id, { height: Number(v) })}
						/>
						<Input
							label="Framerate (fps)"
							type="number"
							className="col-span-2"
							value={opts.fps}
							onChange={(v) => updateTabOptions(tab.id, { fps: Number(v) })}
						/>
					</div>

					<div className="space-y-4 pt-4 border-t border-border/50">
						<SelectInput
							label="Background Mode"
							value={opts.backgroundMode}
							onChange={(val) => updateTabOptions(tab.id, { backgroundMode: val as any })}
							options={[
								{ label: "Alpha Transparent", value: "transparent" },
								{ label: "Chroma Key Green (#00FF00)", value: "chromaKeyGreen" },
								{ label: "Luma Matte", value: "lumaMatte" },
								{ label: "Custom Solid Color", value: "customColor" },
							]}
						/>

						{opts.backgroundMode === "customColor" && (
							<div className="animate-in slide-in-from-top-2 fade-in duration-200">
								<ColorPicker
									label="Custom Background Color"
									color={opts.backgroundColor}
									onChange={(c) => updateTabOptions(tab.id, { backgroundColor: c })}
								/>
							</div>
						)}

						<div className="space-y-2.5">
							<label className="text-sm font-medium text-foreground">Output Directory</label>
							<div className="flex items-center gap-2">
								<div className="flex-1 flex items-center px-3 py-2 bg-muted/40 border border-input rounded-lg shadow-sm overflow-hidden">
									<span className="text-sm text-muted-foreground truncate select-none">
										{opts.outputPath || "System Default Path"}
									</span>
								</div>
								<button
									onClick={handleSelectOutputDir}
									className="px-4 py-2 bg-secondary text-secondary-foreground text-sm font-medium rounded-lg hover:bg-secondary/80 transition-colors shadow-sm"
								>
									Browse
								</button>
							</div>
						</div>
					</div>
				</Section>

				{/* --- 2. TYPOGRAPHY & TEXT STYLES --- */}
				<Section title="Typography" icon={<Type />} description="Fonts, spacing, and colors">
					<div className="grid grid-cols-2 gap-4">
						<Input
							label="Font Family"
							value={opts.fontName}
							className="col-span-2"
							onChange={(v) => updateTabOptions(tab.id, { fontName: v })}
						/>
						<Input
							label="Font Size (px)"
							type="number"
							value={opts.fontSize}
							onChange={(v) => updateTabOptions(tab.id, { fontSize: Number(v) })}
						/>
						<Input
							label="Line Spacing"
							type="number"
							value={opts.lineSpacing}
							onChange={(v) => updateTabOptions(tab.id, { lineSpacing: Number(v) })}
						/>
						<Input
							label="Message Spacing"
							type="number"
							value={opts.messageSpacing}
							onChange={(v) => updateTabOptions(tab.id, { messageSpacing: Number(v) })}
						/>
						<Input
							label="Outer Padding"
							type="number"
							value={opts.padding}
							onChange={(v) => updateTabOptions(tab.id, { padding: Number(v) })}
						/>
					</div>

					<div className="pt-4 mt-2 border-t border-border/50">
						<ColorPicker
							label="Global Text Color"
							color={opts.messageColor}
							onChange={(c) => updateTabOptions(tab.id, { messageColor: c })}
						/>
					</div>

					<div className="mt-4 space-y-4 p-4 bg-muted/30 rounded-xl border border-border/50">
						<Toggle
							label="Apply Drop Shadow"
							description="Add a subtle shadow behind usernames"
							checked={opts.usernameShadow}
							onChange={(c) => updateTabOptions(tab.id, { usernameShadow: c })}
						/>
						<Toggle
							label="Outline Usernames"
							checked={opts.outlineUsernames}
							onChange={(c) => updateTabOptions(tab.id, { outlineUsernames: c })}
						/>
						{opts.outlineUsernames && (
							<div className="pt-2 pl-2 border-l-2 border-primary/20 animate-in slide-in-from-left-2 fade-in duration-200">
								<OptionalNumberInput
									label="Outline Stroke Width"
									value={opts.usernameOutlineWidth}
									onChange={(v) => updateTabOptions(tab.id, { usernameOutlineWidth: v })}
								/>
							</div>
						)}
					</div>
				</Section>

				{/* --- 3. MESSAGE BUBBLES --- */}
				<Section title="Message Bubbles" icon={<LayoutTemplate />} description="Chat background styling">
					<Toggle
						label="Enable Message Bubbles"
						description="Wrap messages in a colored background"
						checked={opts.bubbleModeFullWidth}
						onChange={(c) => updateTabOptions(tab.id, { bubbleModeFullWidth: c })}
					/>

					{opts.bubbleModeFullWidth && (
						<div className="mt-5 space-y-5 p-5 bg-muted/30 rounded-xl border border-border/50 animate-in slide-in-from-top-2 fade-in duration-200">
							<div className="grid grid-cols-2 gap-4">
								<Input
									label="Corner Radius"
									type="number"
									value={opts.bubbleRadius}
									onChange={(v) => updateTabOptions(tab.id, { bubbleRadius: Number(v) })}
								/>
								<Input
									label="Inner Padding"
									type="number"
									value={opts.bubblePadding}
									onChange={(v) => updateTabOptions(tab.id, { bubblePadding: Number(v) })}
								/>
							</div>
							<ColorPicker
								label="Bubble Fill Color"
								color={opts.bubbleColor}
								onChange={(c) => updateTabOptions(tab.id, { bubbleColor: c })}
							/>
						</div>
					)}
				</Section>

				{/* --- 4. TIMING & OFFSETS --- */}
				<Section title="Timeline & Offsets" icon={<Clock />} description="Synchronize chat with video">
					<div className="grid grid-cols-1 gap-5">
						<OptionalNumberInput
							label="Start Offset (ms)"
							value={opts.startMs}
							onChange={(v) => updateTabOptions(tab.id, { startMs: v })}
						/>
						<OptionalNumberInput
							label="End Offset (ms)"
							value={opts.endMs}
							onChange={(v) => updateTabOptions(tab.id, { endMs: v })}
						/>
						<OptionalNumberInput
							label="Time Zero (ms)"
							value={opts.timeZeroMs}
							onChange={(v) => updateTabOptions(tab.id, { timeZeroMs: v })}
						/>
					</div>
				</Section>

				{/* --- 5. ANIMATION & EVICTION --- */}
				<Section title="Animation Behavior" icon={<Settings />} description="Entry and exit transitions">
					<SelectInput
						label="Eviction Strategy"
						value={opts.evictionStrategy}
						onChange={(val) => updateTabOptions(tab.id, { evictionStrategy: val as any })}
						options={[
							{ label: "Timed (Disappear after hold)", value: "timed" },
							{ label: "Push Only (Scroll off screen)", value: "pushOnly" },
						]}
					/>

					<div className="grid grid-cols-2 gap-4 mt-5">
						<Input
							label="Hold Time (sec)"
							type="number"
							value={opts.messageHoldSeconds}
							onChange={(v) => updateTabOptions(tab.id, { messageHoldSeconds: Number(v) })}
						/>
						<Input
							label="Fade Time (sec)"
							type="number"
							value={opts.messageFadeOutSeconds}
							onChange={(v) => updateTabOptions(tab.id, { messageFadeOutSeconds: Number(v) })}
						/>
					</div>

					<div className="mt-5 space-y-4 p-4 bg-muted/30 rounded-xl border border-border/50">
						<Toggle
							label="Slide-in Animation"
							checked={opts.animSlide}
							onChange={(c) => updateTabOptions(tab.id, { animSlide: c })}
						/>
						<Toggle
							label="Fade-in Animation"
							checked={opts.animFadeIn}
							onChange={(c) => updateTabOptions(tab.id, { animFadeIn: c })}
						/>
					</div>
				</Section>

				{/* --- 6. USER FILTERS --- */}
				<Section title="User Management" icon={<Users />} description="Filter and pin specific users">
					<div className="space-y-5">
						<ArrayInput
							label="Ignored Users"
							description="Comma separated list of bots or users to skip"
							placeholder="BotRix, Nightbot, StreamElements"
							value={opts.skipUsers}
							onChange={(arr) => updateTabOptions(tab.id, { skipUsers: arr })}
						/>
						<div className="pt-2 border-t border-border/50">
							<ArrayInput
								label="Pinned Users"
								description="Highlight messages from these users"
								placeholder="Moderator1, VIP_User"
								value={opts.pinnedUsers}
								onChange={(arr) => updateTabOptions(tab.id, { pinnedUsers: arr })}
							/>
						</div>
						{opts.pinnedUsers.length > 0 && (
							<div className="animate-in slide-in-from-top-2 fade-in duration-200">
								<Input
									label="Pin Duration (sec)"
									type="number"
									value={opts.pinDurationSecs}
									onChange={(v) => updateTabOptions(tab.id, { pinDurationSecs: Number(v) })}
								/>
							</div>
						)}
					</div>
				</Section>

				{/* --- 7. ADVANCED MESSAGING --- */}
				<Section title="Advanced Features" icon={<MessageSquare />} description="Grouping and emote rendering">
					<div className="space-y-4">
						<Toggle
							label="Group Sequential Messages"
							description="Combine rapid messages from the same user"
							checked={opts.groupMessages}
							onChange={(c) => updateTabOptions(tab.id, { groupMessages: c })}
						/>
						{opts.groupMessages && (
							<div className="pl-2 border-l-2 border-primary/20 animate-in slide-in-from-left-2 fade-in duration-200">
								<Input
									label="Grouping Window (sec)"
									type="number"
									value={opts.groupMessagesWindowSecs}
									onChange={(v) => updateTabOptions(tab.id, { groupMessagesWindowSecs: Number(v) })}
								/>
							</div>
						)}
					</div>

					<div className="mt-6 space-y-4 p-5 bg-muted/30 rounded-xl border border-border/50">
						<div className="flex items-center gap-2 text-foreground pb-3 border-b border-border/50">
							<div className="p-1.5 bg-primary/10 rounded-md">
								<ImageIcon className="w-4 h-4 text-primary" />
							</div>
							<span className="text-sm font-semibold">Emote Engine</span>
						</div>
						<Toggle
							label="Center Vertically"
							checked={opts.centerEmotesVertically}
							onChange={(c) => updateTabOptions(tab.id, { centerEmotesVertically: c })}
						/>
						<Toggle
							label="Premultiplied Alpha"
							checked={opts.cratePremultipliedAlphaEmotes}
							onChange={(c) => updateTabOptions(tab.id, { cratePremultipliedAlphaEmotes: c })}
						/>
					</div>
				</Section>
			</div>

			{/* STICKY BOTTOM BAR */}
			<div className="fixed bottom-0 left-0 right-0 p-4 bg-background/80 backdrop-blur-xl border-t border-border z-40 shadow-[0_-10px_40px_-15px_rgba(0,0,0,0.1)]">
				<div className="max-w-350 mx-auto flex items-center justify-between">
					<div className="hidden md:block text-sm text-muted-foreground font-medium">
						Configure settings and render directly to video.
					</div>
					<button
						onClick={handleQueueRender}
						className="w-full md:w-auto flex items-center justify-center gap-2 rounded-xl bg-primary px-8 py-3.5 text-sm font-bold tracking-wide text-primary-foreground shadow-lg shadow-primary/25 transition-all hover:bg-primary/90 hover:shadow-primary/40 hover:-translate-y-0.5 active:scale-[0.98] focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-background"
					>
						<Play className="h-5 w-5 fill-current" />
						Start Rendering Queue
					</button>
				</div>
			</div>
		</div>
	);
}

// ==========================================
// FORM COMPONENT HELPERS
// ==========================================

function Section({
					 title,
					 icon,
					 description,
					 children,
				 }: {
	title: string;
	icon: React.ReactNode;
	description?: string;
	children: React.ReactNode;
}) {
	return (
		<div className="flex flex-col h-full rounded-2xl border border-border bg-card p-6 shadow-sm transition-shadow hover:shadow-md">
			<div className="flex items-start gap-3 mb-6 shrink-0">
				<div className="p-2.5 bg-primary/10 rounded-xl shrink-0 text-primary [&>svg]:w-5 [&>svg]:h-5">
					{icon}
				</div>
				<div className="space-y-1 mt-0.5">
					<h3 className="text-base font-semibold text-foreground tracking-tight leading-none">{title}</h3>
					{description && <p className="text-sm text-muted-foreground leading-snug">{description}</p>}
				</div>
			</div>
			<div className="flex-1 flex flex-col">{children}</div>
		</div>
	);
}

function Input({
				   label,
				   type = "text",
				   value,
				   className = "",
				   onChange,
			   }: {
	label: string;
	type?: string;
	value: any;
	className?: string;
	onChange: (val: string) => void;
}) {
	return (
		<div className={`space-y-2 ${className}`}>
			<label className="text-sm font-medium leading-none text-foreground peer-disabled:cursor-not-allowed peer-disabled:opacity-70">
				{label}
			</label>
			<input
				type={type}
				className="flex h-10 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:border-ring disabled:cursor-not-allowed disabled:opacity-50"
				value={value}
				onChange={(e) => onChange(e.target.value)}
			/>
		</div>
	);
}

function OptionalNumberInput({
								 label,
								 value,
								 onChange,
							 }: {
	label: string;
	value: number | null;
	onChange: (val: number | null) => void;
}) {
	return (
		<div className="space-y-2">
			<label className="text-sm font-medium leading-none text-foreground">
				{label} <span className="text-muted-foreground font-normal ml-1">(Optional)</span>
			</label>
			<input
				type="number"
				placeholder="Auto"
				className="flex h-10 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm transition-colors placeholder:text-muted-foreground/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:border-ring"
				value={value ?? ""}
				onChange={(e) => {
					const val = e.target.value;
					onChange(val === "" ? null : Number(val));
				}}
			/>
		</div>
	);
}

function SelectInput({
						 label,
						 value,
						 options,
						 onChange,
					 }: {
	label: string;
	value: string;
	options: { label: string; value: string }[];
	onChange: (val: string) => void;
}) {
	return (
		<div className="space-y-2">
			<label className="text-sm font-medium leading-none text-foreground">
				{label}
			</label>
			<select
				className="flex h-10 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:border-ring"
				value={value}
				onChange={(e) => onChange(e.target.value)}
			>
				{options.map((opt) => (
					<option key={opt.value} value={opt.value}>
						{opt.label}
					</option>
				))}
			</select>
		</div>
	);
}

function ArrayInput({
						label,
						description,
						value,
						placeholder,
						onChange,
					}: {
	label: string;
	description?: string;
	value: string[];
	placeholder?: string;
	onChange: (arr: string[]) => void;
}) {
	return (
		<div className="space-y-2">
			<div>
				<label className="text-sm font-medium leading-none text-foreground">
					{label}
				</label>
				{description && <p className="text-xs text-muted-foreground mt-1.5">{description}</p>}
			</div>
			<textarea
				className="flex min-h-20 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm transition-colors placeholder:text-muted-foreground/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:border-ring resize-y"
				value={value.join(", ")}
				placeholder={placeholder}
				onChange={(e) => {
					const arr = e.target.value
						.split(",")
						.map((s) => s.trim())
						.filter(Boolean);
					onChange(arr);
				}}
			/>
		</div>
	);
}

function ColorPicker({
						 label,
						 color,
						 onChange,
						 className = "",
					 }: {
	label: string;
	color: ObjectColor;
	onChange: (c: ObjectColor) => void;
	className?: string;
}) {
	return (
		<div className={`space-y-2 ${className}`}>
			<label className="text-sm font-medium leading-none text-foreground">
				{label}
			</label>
			<div className="flex gap-3 items-center">
				<div className="relative shrink-0 w-12 h-10 rounded-lg overflow-hidden border border-input shadow-sm focus-within:ring-2 focus-within:ring-ring focus-within:border-ring transition-shadow">
					<input
						type="color"
						className="absolute -top-2 -left-2 w-16 h-16 cursor-pointer bg-transparent border-0 p-0"
						value={objToHex(color)}
						onChange={(e) => onChange(hexToObj(e.target.value, color.alpha))}
					/>
				</div>
				<div className="flex-1 relative">
					<span className="absolute left-3 top-1/2 -translate-y-1/2 text-xs font-medium text-muted-foreground pointer-events-none">
						Alpha
					</span>
					<input
						type="number"
						min="0"
						max="255"
						className="flex h-10 w-full rounded-lg border border-input bg-background pl-14 pr-3 py-2 text-sm text-foreground shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:border-ring"
						value={color.alpha}
						onChange={(e) => onChange({ ...color, alpha: Number(e.target.value) })}
					/>
				</div>
			</div>
		</div>
	);
}

function Toggle({
					label,
					description,
					checked,
					onChange,
				}: {
	label: string;
	description?: string;
	checked: boolean;
	onChange: (val: boolean) => void;
}) {
	return (
		<label className="flex flex-row items-center justify-between rounded-lg cursor-pointer group">
			<div className="space-y-0.5 mr-4">
				<div className="text-sm font-medium text-foreground group-hover:text-primary transition-colors">
					{label}
				</div>
				{description && (
					<div className="text-xs text-muted-foreground">
						{description}
					</div>
				)}
			</div>
			<div
				className={`relative inline-flex h-6 w-11 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-background ${
					checked ? "bg-primary" : "bg-input"
				}`}
			>
				<span
					className={`pointer-events-none inline-block h-5 w-5 transform rounded-full bg-background shadow-lg ring-0 transition duration-200 ease-in-out ${
						checked ? "translate-x-5" : "translate-x-0"
					}`}
				/>
			</div>
			<input
				type="checkbox"
				className="sr-only"
				checked={checked}
				onChange={(e) => onChange(e.target.checked)}
			/>
		</label>
	);
}