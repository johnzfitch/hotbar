import GLib from "gi://GLib"
import Gio from "gi://Gio"

// Debug configuration — set via setDebugConfig() from app.tsx
let _debugEnabled = false
let _debugFile: string | null = null
let _debugStream: Gio.DataOutputStream | null = null

export function setDebugConfig(enabled: boolean, filePath: string | null): void {
  _debugEnabled = enabled
  _debugFile = filePath

  if (_debugFile) {
    try {
      const file = Gio.File.new_for_path(_debugFile)
      const stream = file.replace(null, false, Gio.FileCreateFlags.NONE, null)
      _debugStream = new Gio.DataOutputStream({ base_stream: stream })
      _debugStream.put_string(`=== Hotbar debug log started ${new Date().toISOString()} ===\n`, null)
    } catch (e) {
      console.error(`Failed to open debug log file: ${e}`)
      _debugStream = null
    }
  }

  if (_debugEnabled || _debugFile) {
    console.log(`[hotbar] Debug: console=${enabled}, file=${filePath || "none"}`)
  }
}

function debug(category: string, ...args: unknown[]): void {
  if (!_debugEnabled && !_debugFile) return

  const ts = new Date().toISOString().slice(11, 23)
  const msg = `[${ts}] [hotfiles:${category}] ${args.map(a => typeof a === "string" ? a : JSON.stringify(a)).join(" ")}`

  if (_debugEnabled) {
    console.log(msg)
  }

  if (_debugStream) {
    try {
      _debugStream.put_string(msg + "\n", null)
      _debugStream.flush(null)
    } catch {
      // Ignore write errors
    }
  }
}

// Centralized HOME check — hotbar only processes files under ~/
function isUnderHome(path: string): boolean {
  return path.startsWith(HOME + "/") || path === HOME
}

export type Action = "opened" | "modified" | "created" | "deleted"
export type Source = "user" | "claude" | "codex" | "system"
export type Filter = "all" | Source
export type ActionFilter = "all" | Action

export interface HotFile {
  path: string
  filename: string
  dir: string
  fullDir: string
  timestamp: number // absolute Unix seconds
  source: Source
  mimeType: string
  action: Action
  confidence?: "high" | "low" // high = patch-derived, low = heuristic
}

const HOME = GLib.get_home_dir()
const EVENTS_PATH = `${HOME}/.claude/.statusline/events.jsonl`
const XBEL_PATH = `${HOME}/.local/share/recently-used.xbel`
const CODEX_SESSIONS_DIR = `${HOME}/.codex/sessions`
const SYSTEM_PATH_PREFIXES = [
  `${HOME}/.codex/`,
  `${HOME}/.claude/`,
]
const INCLUDE_SYSTEM_EVENTS = (() => {
  const raw = GLib.getenv("HOTBAR_INCLUDE_SYSTEM_EVENTS")
  if (!raw) return false
  const v = raw.trim().toLowerCase()
  return v === "1" || v === "true" || v === "yes" || v === "on"
})()

const TOOL_ACTIONS = new Set(["Write", "Edit", "NotebookEdit", "Read"])

// MIME types we care about from xbel (code/text files)
const RELEVANT_MIME_PREFIXES = [
  "text/",
  "application/json",
  "application/javascript",
  "application/typescript",
  "application/xml",
  "application/x-shellscript",
  "application/x-python",
  "application/toml",
  "application/yaml",
  "application/x-ruby",
  "application/sql",
  "application/x-perl",
]

// Directories to skip during scanning
const SKIP_DIRS = new Set([
  "node_modules", ".git", ".venv", "__pycache__", "target",
  "dist", "build", ".next", ".cache", ".vite",
])

// Files to skip during scanning
const SKIP_FILES = new Set([
  "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
  "Cargo.lock", "flake.lock",
])

function isRelevantMime(mime: string): boolean {
  return RELEVANT_MIME_PREFIXES.some((p) => mime.startsWith(p))
}

function readFileText(path: string): string | null {
  try {
    const file = Gio.File.new_for_path(path)
    const [ok, contents] = file.load_contents(null)
    if (!ok || !contents) return null
    return new TextDecoder().decode(contents)
  } catch {
    return null
  }
}

function getFileMtime(path: string): number {
  try {
    const file = Gio.File.new_for_path(path)
    const info = file.query_info("time::modified", Gio.FileQueryInfoFlags.NONE, null)
    return info.get_attribute_uint64("time::modified")
  } catch {
    return 0
  }
}

function fileExists(path: string): boolean {
  return GLib.file_test(path, GLib.FileTest.EXISTS)
}

function guessMimeType(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() || ""
  const map: Record<string, string> = {
    ts: "text/typescript",
    tsx: "text/typescript",
    js: "text/javascript",
    jsx: "text/javascript",
    py: "text/x-python",
    rs: "text/x-rust",
    go: "text/x-go",
    rb: "text/x-ruby",
    sh: "application/x-shellscript",
    bash: "application/x-shellscript",
    zsh: "application/x-shellscript",
    json: "application/json",
    toml: "application/toml",
    yaml: "application/yaml",
    yml: "application/yaml",
    md: "text/markdown",
    txt: "text/plain",
    html: "text/html",
    css: "text/css",
    scss: "text/x-scss",
    sql: "application/sql",
    xml: "application/xml",
    nix: "text/x-nix",
    lua: "text/x-lua",
    c: "text/x-c",
    h: "text/x-c",
    cpp: "text/x-c++",
    hpp: "text/x-c++",
    java: "text/x-java",
    kt: "text/x-kotlin",
    swift: "text/x-swift",
    conf: "text/plain",
    cfg: "text/plain",
    ini: "text/plain",
    env: "text/plain",
    lock: "text/plain",
    php: "text/x-php",
    vue: "text/x-vue",
    svelte: "text/x-svelte",
  }
  return map[ext] || "text/plain"
}

function isCodeFile(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() || ""
  const mime = guessMimeType(path)
  // Accept anything that's text-like or has a known code extension
  return mime !== "text/plain" || [
    "txt", "conf", "cfg", "ini", "env", "lock",
    "gitignore", "dockerignore", "editorconfig",
  ].includes(ext)
}

function isSystemPath(path: string): boolean {
  return SYSTEM_PATH_PREFIXES.some((prefix) => path.startsWith(prefix))
}

function sourceForPath(path: string, nonSystemSource: Exclude<Source, "system">): Source | null {
  if (!isSystemPath(path)) return nonSystemSource
  return INCLUDE_SYSTEM_EVENTS ? "system" : null
}

function pathParts(path: string): { filename: string; dir: string } {
  const lastSlash = path.lastIndexOf("/")
  if (lastSlash === -1) return { filename: path, dir: "" }
  return {
    filename: path.slice(lastSlash + 1),
    dir: path.slice(0, lastSlash),
  }
}

// Shorten dir for display: replace home with ~, truncate middle
function shortenDir(dir: string): string {
  if (dir.startsWith(HOME)) {
    dir = "~" + dir.slice(HOME.length)
  }
  if (dir.length > 40) {
    const parts = dir.split("/")
    if (parts.length > 4) {
      return parts.slice(0, 2).join("/") + "/.../" + parts.slice(-2).join("/")
    }
  }
  return dir
}

class HotFilesService {
  private static _instance: HotFilesService | null = null
  private _files: HotFile[] = []
  private _filter: Filter = "all"
  private _actionFilter: ActionFilter = "all"
  private _listeners: Set<() => void> = new Set()
  private _pollTimer: number | null = null
  private _sourceMonitors: Gio.FileMonitor[] = []
  private _dirMonitors: Map<string, Gio.FileMonitor> = new Map()
  private _debounceTimer: number | null = null
  // Track known paths for create detection across refreshes
  private _knownPaths: Set<string> = new Set()
  private _initialized = false

  static get_default(): HotFilesService {
    if (!this._instance) {
      this._instance = new HotFilesService()
    }
    return this._instance
  }

  private constructor() {
    debug("init", `HOME=${HOME}`)
    debug("init", `EVENTS_PATH=${EVENTS_PATH}`)
    debug("init", `XBEL_PATH=${XBEL_PATH}`)
    debug("init", `HOTBAR_INCLUDE_SYSTEM_EVENTS=${INCLUDE_SYSTEM_EVENTS}`)
    this._refresh()
    this._watchSourceFiles()
    // 30s fallback poll — catches anything the file monitors miss
    this._pollTimer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 30000, () => {
      debug("poll", "30s fallback poll triggered")
      this._refresh()
      return GLib.SOURCE_CONTINUE
    })
    debug("init", "Service initialized")
  }

  get files(): HotFile[] {
    let result = this._files
    if (this._filter !== "all") {
      result = result.filter((f) => f.source === this._filter)
    }
    if (this._actionFilter !== "all") {
      result = result.filter((f) => f.action === this._actionFilter)
    }
    return result
  }

  get allFiles(): HotFile[] {
    return this._files
  }

  get filter(): Filter {
    return this._filter
  }

  get actionFilter(): ActionFilter {
    return this._actionFilter
  }

  setFilter(f: Filter) {
    this._filter = f
    this._notify()
  }

  setActionFilter(f: ActionFilter) {
    this._actionFilter = f
    this._notify()
  }

  subscribe(cb: () => void): () => void {
    this._listeners.add(cb)
    return () => this._listeners.delete(cb)
  }

  forceRefresh() {
    this._refresh()
  }

  private _notify() {
    this._listeners.forEach((cb) => cb())
  }

  private _debounceRefresh() {
    if (this._debounceTimer !== null) {
      GLib.source_remove(this._debounceTimer)
      debug("debounce", "Reset debounce timer")
    }
    this._debounceTimer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 300, () => {
      debug("debounce", "Debounce fired, triggering refresh")
      this._refresh()
      this._debounceTimer = null
      return GLib.SOURCE_REMOVE
    })
  }

  // Watch the two primary data source files + Codex session directory
  private _watchSourceFiles(): void {
    const paths = [EVENTS_PATH, XBEL_PATH]
    for (const path of paths) {
      try {
        const file = Gio.File.new_for_path(path)
        const parent = file.get_parent()
        if (parent && !parent.query_exists(null)) {
          debug("watch", `Skipping ${path} — parent dir doesn't exist`)
          continue
        }

        const monitor = file.monitor_file(Gio.FileMonitorFlags.NONE, null)
        monitor.connect("changed", (_monitor, _file, _otherFile, eventType) => {
          const eventNames: Record<number, string> = {
            [Gio.FileMonitorEvent.CHANGED]: "CHANGED",
            [Gio.FileMonitorEvent.CREATED]: "CREATED",
            [Gio.FileMonitorEvent.DELETED]: "DELETED",
            [Gio.FileMonitorEvent.CHANGES_DONE_HINT]: "CHANGES_DONE_HINT",
          }
          debug("watch", `${path} event: ${eventNames[eventType] || eventType}`)
          if (
            eventType === Gio.FileMonitorEvent.CHANGED ||
            eventType === Gio.FileMonitorEvent.CREATED ||
            eventType === Gio.FileMonitorEvent.CHANGES_DONE_HINT
          ) {
            this._debounceRefresh()
          }
        })
        this._sourceMonitors.push(monitor)
        debug("watch", `Watching ${path}`)
      } catch (e) {
        console.warn(`Failed to watch ${path}: ${e}`)
      }
    }

    // Watch today's Codex session directory for new/updated session files
    this._watchCodexSessionDir()
  }

  private _watchCodexSessionDir(): void {
    const now = GLib.DateTime.new_now_local()
    const y = now.get_year().toString().padStart(4, "0")
    const m = now.get_month().toString().padStart(2, "0")
    const d = now.get_day_of_month().toString().padStart(2, "0")
    const todayDir = `${CODEX_SESSIONS_DIR}/${y}/${m}/${d}`

    try {
      const dirFile = Gio.File.new_for_path(todayDir)
      if (!dirFile.query_exists(null)) {
        debug("watch", `Codex session dir doesn't exist yet: ${todayDir}`)
        return
      }
      const monitor = dirFile.monitor_directory(Gio.FileMonitorFlags.NONE, null)
      monitor.connect("changed", (_m, changedFile) => {
        const name = changedFile?.get_basename() || ""
        if (name.endsWith(".jsonl")) {
          debug("watch", `Codex session change: ${name}`)
          this._debounceRefresh()
        }
      })
      this._sourceMonitors.push(monitor)
      debug("watch", `Watching Codex session dir: ${todayDir}`)
    } catch (e) {
      debug("watch", `Failed to watch Codex session dir: ${e}`)
    }
  }

  // Monitor active project directories for user file changes
  // Cap at 10 to stay well within FD limits (each monitor uses inotify resources)
  private _updateDirMonitors(activeDirs: Set<string>): void {
    if (this._dirMonitors.size >= 10) {
      debug("dirmon", `At monitor cap (10), skipping new monitors`)
      return
    }
    for (const dir of activeDirs) {
      if (this._dirMonitors.size >= 10) {
        debug("dirmon", `Hit monitor cap, stopping`)
        break
      }
      if (this._dirMonitors.has(dir)) continue

      // Only monitor under ~/
      if (!isUnderHome(dir)) {
        debug("dirmon", `Skip dir (outside ~): ${dir}`)
        continue
      }

      try {
        const file = Gio.File.new_for_path(dir)
        if (!file.query_exists(null)) {
          debug("dirmon", `Dir doesn't exist: ${dir}`)
          continue
        }

        const monitor = file.monitor_directory(Gio.FileMonitorFlags.NONE, null)
        monitor.connect("changed", (_m, changedFile) => {
          const changedPath = changedFile?.get_path() || "unknown"
          debug("dirmon", `Dir change in ${dir}: ${changedPath}`)
          this._debounceRefresh()
        })
        this._dirMonitors.set(dir, monitor)
        debug("dirmon", `Monitoring dir: ${dir} (total: ${this._dirMonitors.size})`)
      } catch {
        continue
      }
    }
  }

  private _refresh() {
    const startTime = Date.now()
    debug("refresh", "=== Starting refresh ===")

    const claudeFiles = this._readAgentFiles()
    debug("refresh", `Claude files: ${claudeFiles.length}`)

    const xbelFiles = this._readXbelFiles()
    debug("refresh", `XBEL files: ${xbelFiles.length}`)

    const codexFiles = this._readCodexFiles()
    debug("refresh", `Codex files: ${codexFiles.length}`)

    // Get active directories from Claude + Codex events for scanning + monitoring
    const activeDirs = new Set<string>()
    for (const f of claudeFiles) {
      activeDirs.add(f.fullDir)
    }
    for (const f of codexFiles) {
      activeDirs.add(f.fullDir)
    }
    debug("refresh", `Active dirs: ${activeDirs.size}`)

    // Scan those directories for user-modified files, excluding recent agent writes
    const agentFiles = [...claudeFiles, ...codexFiles]
    const dirFiles = this._scanActiveDirs(activeDirs, agentFiles)
    debug("refresh", `Dir-scanned files: ${dirFiles.length}`)

    // Set up directory monitors for real-time updates
    this._updateDirMonitors(activeDirs)

    // Merge all sources: deduplicate by (path, source), keep most recent
    const byKey = new Map<string, HotFile>()

    // Event-backed files
    for (const f of claudeFiles) {
      const key = `${f.path}:${f.source}`
      const existing = byKey.get(key)
      if (!existing || f.timestamp > existing.timestamp) {
        byKey.set(key, f)
      }
    }

    // Codex patch-derived files
    for (const f of codexFiles) {
      const key = `${f.path}:${f.source}`
      const existing = byKey.get(key)
      if (!existing || f.timestamp > existing.timestamp) {
        byKey.set(key, f)
      }
    }

    // xbel files — only add if not already covered by dir scan
    for (const f of xbelFiles) {
      const key = `${f.path}:${f.source}`
      const existing = byKey.get(key)
      if (!existing || f.timestamp > existing.timestamp) {
        byKey.set(key, f)
      }
    }

    // Directory-scanned files — these are most accurate for on-disk state
    for (const f of dirFiles) {
      const key = `${f.path}:${f.source}`
      const existing = byKey.get(key)
      if (!existing || f.timestamp > existing.timestamp) {
        byKey.set(key, f)
      }
    }

    // Check for deleted files
    for (const [_key, f] of byKey) {
      if (!fileExists(f.path)) {
        f.action = "deleted"
        // Remove from known paths so if the file reappears, it's detected as "created"
        this._knownPaths.delete(f.path)
      }
    }

    // Detect newly-appeared files (created) on subsequent refreshes
    if (this._initialized) {
      for (const [_key, f] of byKey) {
        if (!this._knownPaths.has(f.path) && f.action !== "deleted" && f.action !== "opened") {
          f.action = "created"
        }
      }
    }

    // Accumulate known paths (don't replace — ensures "created" detection persists)
    for (const [_key, f] of byKey) {
      this._knownPaths.add(f.path)
    }
    this._initialized = true

    const merged = Array.from(byKey.values())
      .sort((a, b) => b.timestamp - a.timestamp)
      .slice(0, 200)

    debug("refresh", `Merged total: ${merged.length} files`)

    // Only notify if data actually changed
    const fingerprint = (files: HotFile[]) =>
      files.map((f) => `${f.path}:${f.source}:${f.timestamp}:${f.action}`).join("|")

    const changed = fingerprint(merged) !== fingerprint(this._files)
    if (changed) {
      this._files = merged
      this._notify()
      debug("refresh", `Data changed, notified ${this._listeners.size} listeners`)

      // Log first 5 files for visibility
      for (let i = 0; i < Math.min(5, merged.length); i++) {
        const f = merged[i]
        debug("refresh", `  [${i}] ${f.source}:${f.action} ${f.path}`)
      }
      if (merged.length > 5) {
        debug("refresh", `  ... and ${merged.length - 5} more`)
      }
    } else {
      debug("refresh", "No data change, skipping notification")
    }

    debug("refresh", `=== Refresh complete in ${Date.now() - startTime}ms ===`)
  }

  // Scan active directories for recently-modified files not covered by agent events
  private _scanActiveDirs(activeDirs: Set<string>, agentFiles: HotFile[]): HotFile[] {
    debug("dirscan", `Scanning ${activeDirs.size} active directories...`)
    const now = GLib.DateTime.new_now_local().to_unix()
    const cutoff = now - 86400 // 24 hours

    // Build map of agent file paths -> latest agent timestamp
    const agentLatest = new Map<string, number>()
    for (const f of agentFiles) {
      const existing = agentLatest.get(f.path) || 0
      if (f.timestamp > existing) agentLatest.set(f.path, f.timestamp)
    }

    const files: HotFile[] = []
    const seen = new Set<string>()
    let dirsScanned = 0
    let filesChecked = 0

    for (const dir of activeDirs) {
      // Only scan under ~/
      if (!isUnderHome(dir)) {
        debug("dirscan", `Skip dir (outside ~): ${dir}`)
        continue
      }

      // Skip hidden/build directories
      const dirName = dir.split("/").pop() || ""
      if (SKIP_DIRS.has(dirName)) {
        debug("dirscan", `Skip dir (in SKIP_DIRS): ${dir}`)
        continue
      }

      try {
        const dirFile = Gio.File.new_for_path(dir)
        if (!dirFile.query_exists(null)) {
          debug("dirscan", `Skip dir (doesn't exist): ${dir}`)
          continue
        }

        dirsScanned++
        const enumerator = dirFile.enumerate_children(
          "standard::name,standard::type,time::modified,time::created",
          Gio.FileQueryInfoFlags.NONE,
          null,
        )

        let info: Gio.FileInfo | null
        while ((info = enumerator.next_file(null)) !== null) {
          filesChecked++
          if (info.get_file_type() !== Gio.FileType.REGULAR) continue

          const name = info.get_name()
          if (!name || name.startsWith(".")) continue
          if (SKIP_FILES.has(name)) continue

          const path = `${dir}/${name}`
          if (seen.has(path)) continue
          seen.add(path)

          const source = sourceForPath(path, "user")
          if (!source) continue

          const mtime = info.get_attribute_uint64("time::modified")
          if (mtime < cutoff) continue

          // Skip if an agent has a recent event for this file
          // 5s tolerance: Claude's absolute timestamps are derived from fileMtime math
          // and can drift slightly vs actual mtime
          const agentTs = agentLatest.get(path)
          if (agentTs && agentTs >= mtime - 5) continue

          // Only include code/text files
          if (!isCodeFile(path)) continue

          // Detect creates: if file birthtime is recent and close to mtime, it was just created
          const birthtime = info.get_attribute_uint64("time::created")
          let action: Action
          if (birthtime > 0 && birthtime > cutoff && (mtime - birthtime) < 120) {
            action = "created"
          } else {
            action = "modified"
          }

          const { filename, dir: dirStr } = pathParts(path)
          files.push({
            path,
            filename,
            dir: shortenDir(dirStr),
            fullDir: dirStr,
            timestamp: mtime,
            source,
            mimeType: guessMimeType(path),
            action,
          })
        }

        // Explicitly close to release FD
        enumerator.close(null)
      } catch (e) {
        debug("dirscan", `Error scanning ${dir}: ${e}`)
        continue
      }
    }

    debug("dirscan", `Scanned ${dirsScanned} dirs, checked ${filesChecked} files, found ${files.length} user files`)
    return files
  }

  private _readAgentFiles(): HotFile[] {
    debug("agent", "Reading events.jsonl...")
    const text = readFileText(EVENTS_PATH)
    if (!text) {
      debug("agent", "events.jsonl not found or empty")
      return []
    }

    const fileMtime = getFileMtime(EVENTS_PATH)
    if (fileMtime === 0) {
      debug("agent", "Could not get file mtime")
      return []
    }

    const lines = text.trim().split("\n")
    if (lines.length === 0) {
      debug("agent", "No lines in events.jsonl")
      return []
    }
    debug("agent", `Total lines: ${lines.length}, file mtime: ${fileMtime}`)

    // events.jsonl accumulates across concurrent + sequential sessions.
    // Timestamps are relative to each session's start. Detect boundaries
    // (where timestamps decrease by >60s) and process ALL recent sessions,
    // computing per-session baseTimes.
    const sessionBoundaries: number[] = [0] // First session starts at line 0
    let prevEvtTs = 0
    for (let i = 0; i < lines.length; i++) {
      try {
        const evt = JSON.parse(lines[i])
        const ts = evt.timestamp ?? 0
        if (typeof ts === "number" && ts < prevEvtTs - 60) {
          sessionBoundaries.push(i)
        }
        if (typeof ts === "number") prevEvtTs = ts
      } catch {
        continue
      }
    }
    debug("agent", `Session boundaries: ${sessionBoundaries.length} sessions detected`)

    // Build session segments: [{startIdx, endIdx, maxRelativeTs}]
    // The LAST session's maxRelativeTs anchors to fileMtime.
    // Earlier sessions' baseTimes are derived by walking backwards.
    const segments: { start: number; end: number; maxTs: number }[] = []
    for (let s = 0; s < sessionBoundaries.length; s++) {
      const start = sessionBoundaries[s]
      const end = s + 1 < sessionBoundaries.length ? sessionBoundaries[s + 1] : lines.length
      let maxTs = 0
      for (let i = end - 1; i >= start; i--) {
        try {
          const evt = JSON.parse(lines[i])
          const ts = evt.timestamp ?? 0
          if (typeof ts === "number" && ts > maxTs) {
            maxTs = ts
            break
          }
        } catch {
          continue
        }
      }
      segments.push({ start, end, maxTs })
    }

    // Compute baseTime for the last segment from fileMtime, then derive
    // earlier segments. The last segment's events are the most recent writes.
    const now = GLib.DateTime.new_now_local().to_unix()
    const lastSeg = segments[segments.length - 1]
    const lastBaseTime = fileMtime - lastSeg.maxTs

    // For each segment, compute baseTime. The last segment uses fileMtime.
    // Earlier segments: we estimate baseTime from the last event's absolute
    // position. Since we don't have wall-clock anchors for earlier segments,
    // we use the heuristic that the first event of the next segment happened
    // right after the boundary was detected.
    const segBaseTimes: number[] = new Array(segments.length)
    segBaseTimes[segments.length - 1] = lastBaseTime

    for (let s = segments.length - 2; s >= 0; s--) {
      // The next segment's first event timestamp gives us a rough anchor:
      // nextSegBaseTime + nextFirstTs ≈ when the next session started
      // thisSegBaseTime + thisMaxTs ≈ when this session last wrote before next started
      // So: thisBaseTime ≈ nextBaseTime + nextFirstTs - thisMaxTs
      // But this can drift. Simpler: use fileMtime - cumulative max timestamps.
      // Actually safest: each segment's baseTime = (next segment's absolute start) - thisMaxTs
      const nextSeg = segments[s + 1]
      let nextFirstTs = 0
      for (let i = nextSeg.start; i < Math.min(nextSeg.start + 10, nextSeg.end); i++) {
        try {
          const evt = JSON.parse(lines[i])
          const ts = evt.timestamp ?? 0
          if (typeof ts === "number" && ts > 0) {
            nextFirstTs = ts
            break
          }
        } catch {
          continue
        }
      }
      const nextAbsStart = segBaseTimes[s + 1] + nextFirstTs
      segBaseTimes[s] = nextAbsStart - segments[s].maxTs
    }

    // Log session info
    const cutoff = now - 86400
    let recentSessions = 0
    for (let s = 0; s < segments.length; s++) {
      const absEnd = segBaseTimes[s] + segments[s].maxTs
      if (absEnd >= cutoff) recentSessions++
    }
    debug("agent", `Recent sessions (24h): ${recentSessions}/${segments.length}`)

    // Track seen paths for created vs modified detection
    const seenPaths = new Set<string>()
    const seen = new Map<string, HotFile>()
    const createdPaths = new Set<string>()

    const filterReasons: Record<string, number> = {}
    const addFilterReason = (reason: string) => {
      filterReasons[reason] = (filterReasons[reason] || 0) + 1
    }

    // Process events from ALL sessions within the 24h window
    let eventsProcessed = 0
    for (let s = 0; s < segments.length; s++) {
      const seg = segments[s]
      const baseTime = segBaseTimes[s]
      const segAbsEnd = baseTime + seg.maxTs

      // Skip sessions that ended more than 24h ago
      if (segAbsEnd < cutoff) {
        addFilterReason("old_session")
        continue
      }

    for (let i = seg.start; i < seg.end; i++) {
      const line = lines[i]
      try {
        const evt = JSON.parse(line)
        if (!TOOL_ACTIONS.has(evt.tool)) {
          addFilterReason("not_tool_action")
          continue
        }
        if (!evt.original_cmd) {
          addFilterReason("no_original_cmd")
          continue
        }

        const path = evt.original_cmd
        if (!isUnderHome(path)) {
          addFilterReason("outside_home")
          debug("agent:filter", `SKIP outside ~: ${path}`)
          continue
        }
        const source = sourceForPath(path, "claude")
        if (!source) {
          addFilterReason("system_path")
          continue
        }
        if (path.includes("/tmp/") || path.includes("node_modules/")) {
          addFilterReason("tmp_or_nodemod")
          continue
        }
        if (path.includes("/dist/") || path.includes("/build/")) {
          addFilterReason("dist_or_build")
          debug("agent:filter", `SKIP dist/build: ${path}`)
          continue
        }

        // Skip lock files and minified bundles
        const basename = path.split("/").pop() || ""
        if (SKIP_FILES.has(basename)) {
          addFilterReason("skip_file")
          continue
        }
        if (basename.endsWith(".min.js") || basename.endsWith(".bundle.js")) {
          addFilterReason("minified")
          debug("agent:filter", `SKIP minified: ${path}`)
          continue
        }

        const absoluteTs = baseTime + evt.timestamp

        // Safety: skip events with obviously wrong timestamps
        if (absoluteTs > now + 60 || absoluteTs < now - 7 * 86400) {
          addFilterReason("bad_timestamp")
          debug("agent:filter", `SKIP bad_ts: ${path} (ts=${absoluteTs}, now=${now})`)
          continue
        }

        eventsProcessed++

        // Determine action
        let action: Action
        if (evt.tool === "Read") {
          action = "opened"
        } else if (evt.tool === "Write") {
          if (!seenPaths.has(path)) {
            action = "created"
            createdPaths.add(path)
          } else {
            action = "modified"
          }
        } else {
          // Edit, NotebookEdit
          action = "modified"
        }
        seenPaths.add(path)

        const existing = seen.get(path)
        if (existing && existing.timestamp >= absoluteTs) continue

        const { filename, dir } = pathParts(path)
        seen.set(path, {
          path,
          filename,
          dir: shortenDir(dir),
          fullDir: dir,
          timestamp: absoluteTs,
          source,
          mimeType: guessMimeType(path),
          action,
        })
      } catch {
        continue
      }
    }
    } // end segment loop

    // Preserve "created" lifecycle: if the first write-type event for a path was a create,
    // keep it as "created" even if later Edit events changed it to "modified"
    for (const [path, file] of seen) {
      if (createdPaths.has(path) && file.action === "modified") {
        file.action = "created"
      }
    }

    debug("agent", `Events processed: ${eventsProcessed}, unique paths: ${seen.size}`)
    debug("agent", `Filter stats: ${JSON.stringify(filterReasons)}`)

    return Array.from(seen.values())
  }

  private _readXbelFiles(): HotFile[] {
    debug("xbel", "Reading recently-used.xbel...")
    const text = readFileText(XBEL_PATH)
    if (!text) {
      debug("xbel", "xbel file not found or empty")
      return []
    }

    const now = GLib.DateTime.new_now_local().to_unix()
    const cutoff = now - 86400 // Only include entries from last 24h

    const files: HotFile[] = []
    const mimeRe = /<mime:mime-type\s+type="([^"]+)"/
    const blocks = text.split("<bookmark ")
    debug("xbel", `Total bookmark blocks: ${blocks.length - 1}`)

    // Track filter reasons
    const filterReasons: Record<string, number> = {}
    const addFilterReason = (reason: string) => {
      filterReasons[reason] = (filterReasons[reason] || 0) + 1
    }

    for (let i = 1; i < blocks.length; i++) {
      const block = "<bookmark " + blocks[i]
      const hrefMatch = block.match(/href="([^"]+)"/)
      const visitedMatch = block.match(/visited="([^"]+)"/)
      const mimeMatch = block.match(mimeRe)

      if (!hrefMatch || !visitedMatch) {
        addFilterReason("no_href_or_visited")
        continue
      }

      const href = hrefMatch[1]
      if (!href.startsWith("file://")) {
        addFilterReason("not_file_uri")
        continue
      }

      // Decode URI
      const path = decodeURIComponent(href.slice(7))
      if (!isUnderHome(path)) {
        addFilterReason("outside_home")
        debug("xbel:filter", `SKIP outside ~: ${path}`)
        continue
      }
      const source = sourceForPath(path, "user")
      if (!source) {
        addFilterReason("system_path")
        continue
      }

      // Some XBEL entries have missing/incorrect MIME (seen on real code files).
      // Fall back to path-based code-file detection so valid opens are not dropped.
      const mime = mimeMatch ? mimeMatch[1] : ""
      const mimeRelevant = mime ? isRelevantMime(mime) : false
      const pathLooksCode = isCodeFile(path)
      if (!mimeRelevant && !pathLooksCode) {
        addFilterReason("irrelevant_mime")
        continue
      }
      const effectiveMime = mimeRelevant ? mime : guessMimeType(path)

      // Skip sandbox/test paths, build artifacts, lock files
      if (path.includes("/dist/") || path.includes("/build/") || path.includes("/node_modules/")) {
        addFilterReason("dist_build_nodemod")
        debug("xbel:filter", `SKIP dist/build/nodemod: ${path}`)
        continue
      }
      const basename = path.split("/").pop() || ""
      if (SKIP_FILES.has(basename)) {
        addFilterReason("skip_file")
        continue
      }
      if (basename.endsWith(".min.js") || basename.endsWith(".bundle.js")) {
        addFilterReason("minified")
        debug("xbel:filter", `SKIP minified: ${path}`)
        continue
      }

      // Parse ISO 8601 timestamp
      const visited = new Date(visitedMatch[1])
      const timestamp = Math.floor(visited.getTime() / 1000)
      if (isNaN(timestamp)) {
        addFilterReason("bad_timestamp")
        continue
      }

      // Skip entries older than 24h
      if (timestamp < cutoff) {
        addFilterReason("too_old")
        continue
      }

      // Don't skip deleted files — the deleted detection in _refresh() will handle them
      const { filename, dir } = pathParts(path)
      files.push({
        path,
        filename,
        dir: shortenDir(dir),
        fullDir: dir,
        timestamp,
        source,
        mimeType: effectiveMime,
        action: "opened",
      })
    }

    debug("xbel", `Files accepted: ${files.length}`)
    debug("xbel", `Filter stats: ${JSON.stringify(filterReasons)}`)

    return files
  }

  // Find recent Codex session JSONL files (today + yesterday, mtime-sorted, capped at 20)
  private _findCodexSessionFiles(): string[] {
    const now = GLib.DateTime.new_now_local()
    const dates: string[] = []
    for (let dayOffset = 0; dayOffset <= 1; dayOffset++) {
      const dt = now.add_days(-dayOffset)
      if (!dt) continue
      const y = dt.get_year().toString().padStart(4, "0")
      const m = dt.get_month().toString().padStart(2, "0")
      const d = dt.get_day_of_month().toString().padStart(2, "0")
      dates.push(`${CODEX_SESSIONS_DIR}/${y}/${m}/${d}`)
    }

    const files: { path: string; mtime: number }[] = []
    for (const dir of dates) {
      try {
        const dirFile = Gio.File.new_for_path(dir)
        if (!dirFile.query_exists(null)) continue
        const enumerator = dirFile.enumerate_children(
          "standard::name,time::modified",
          Gio.FileQueryInfoFlags.NONE,
          null,
        )
        let info: Gio.FileInfo | null
        while ((info = enumerator.next_file(null)) !== null) {
          const name = info.get_name()
          if (!name?.endsWith(".jsonl")) continue
          const mtime = info.get_attribute_uint64("time::modified")
          files.push({ path: `${dir}/${name}`, mtime })
        }
        enumerator.close(null)
      } catch {
        continue
      }
    }

    // Sort by mtime descending (most recent first), cap at 20
    files.sort((a, b) => b.mtime - a.mtime)
    return files.slice(0, 20).map((f) => f.path)
  }

  // Parse Codex session JSONL for apply_patch events (writes only — phase 1)
  private _readCodexFiles(): HotFile[] {
    debug("codex", "Reading Codex session files...")
    const sessionFiles = this._findCodexSessionFiles()
    if (sessionFiles.length === 0) {
      debug("codex", "No recent Codex session files found")
      return []
    }
    debug("codex", `Found ${sessionFiles.length} session files`)

    const now = GLib.DateTime.new_now_local().to_unix()
    const cutoff = now - 86400 // 24h window
    const seen = new Map<string, HotFile>()
    let patchesProcessed = 0

    // Regex to extract file paths from patch headers
    const patchHeaderRe = /\*\*\* (Update|Add|Delete) File: (.+)/g

    for (const sessionPath of sessionFiles) {
      const text = readFileText(sessionPath)
      if (!text) continue

      const lines = text.trim().split("\n")
      for (const line of lines) {
        try {
          const evt = JSON.parse(line)
          if (evt.type !== "response_item") continue

          const payload = evt.payload
          if (!payload) continue

          // apply_patch comes as custom_tool_call or function_call
          const isApplyPatch =
            (payload.type === "custom_tool_call" && payload.name === "apply_patch") ||
            (payload.type === "function_call" && payload.name === "apply_patch")
          if (!isApplyPatch) continue

          // Patch text is in 'input' (custom_tool_call) or 'arguments' (function_call)
          let patchText = ""
          if (payload.input) {
            patchText = payload.input
          } else if (payload.arguments) {
            try {
              const args = JSON.parse(payload.arguments)
              patchText = args.patch || args.input || ""
            } catch {
              patchText = payload.arguments
            }
          }
          if (!patchText) continue

          // Parse ISO timestamp from the row
          const rowTs = evt.timestamp ? Math.floor(new Date(evt.timestamp).getTime() / 1000) : 0
          if (rowTs === 0 || isNaN(rowTs)) continue
          if (rowTs < cutoff || rowTs > now + 60) continue

          // Extract file paths from patch headers
          patchHeaderRe.lastIndex = 0
          let match: RegExpExecArray | null
          while ((match = patchHeaderRe.exec(patchText)) !== null) {
            patchesProcessed++
            const operation = match[1] // Update, Add, Delete
            let path = match[2].trim()

            // Resolve relative paths against session cwd if needed
            if (!path.startsWith("/")) {
              // Try to get cwd from session_meta (first line)
              // For now, skip relative paths — they're uncommon in full-access mode
              debug("codex", `Skip relative path: ${path}`)
              continue
            }

            if (!isUnderHome(path)) continue
            const source = sourceForPath(path, "codex")
            if (!source) continue
            if (path.includes("/tmp/") || path.includes("node_modules/")) continue

            const basename = path.split("/").pop() || ""
            if (SKIP_FILES.has(basename)) continue

            let action: Action
            if (operation === "Add") action = "created"
            else if (operation === "Delete") action = "deleted"
            else action = "modified"

            const existing = seen.get(path)
            if (existing && existing.timestamp >= rowTs) continue

            const { filename, dir } = pathParts(path)
            seen.set(path, {
              path,
              filename,
              dir: shortenDir(dir),
              fullDir: dir,
              timestamp: rowTs,
              source,
              mimeType: guessMimeType(path),
              action,
              confidence: "high",
            })
          }
        } catch {
          continue
        }
      }
    }

    debug("codex", `Patches processed: ${patchesProcessed}, unique paths: ${seen.size}`)
    return Array.from(seen.values())
  }
}

export { HotFilesService, shortenDir, pathParts, guessMimeType }
export default HotFilesService
