import app from "ags/gtk4/app"
import GLib from "gi://GLib"
import Gtk from "gi://Gtk?version=4.0"
import Gdk from "gi://Gdk?version=4.0"
import Gio from "gi://Gio"
import Pango from "gi://Pango"
import Astal from "gi://Astal?version=4.0"
import { createState, onCleanup } from "ags"
import HotFilesService, { type HotFile, type Filter, type ActionFilter, type Source } from "../services/hotfiles"

// Panel visibility — shared across monitor instances
let panelVisible = false
const panelListeners: Set<(visible: boolean) => void> = new Set()

export function togglePanel() {
  panelVisible = !panelVisible
  panelListeners.forEach((cb) => cb(panelVisible))
}

export function isPanelVisible() {
  return panelVisible
}

function timeAgo(timestamp: number): string {
  const now = GLib.DateTime.new_now_local().to_unix()
  const diff = now - timestamp
  if (diff < 0) return "just now"
  if (diff < 60) return "just now"
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  if (diff < 172800) return "yesterday"
  return `${Math.floor(diff / 86400)}d ago`
}

function getIconForMime(mime: string): string {
  if (mime.startsWith("text/x-python") || mime === "application/x-python")
    return "text-x-python-symbolic"
  if (mime.includes("javascript") || mime.includes("typescript"))
    return "text-x-script-symbolic"
  if (mime.includes("shell")) return "utilities-terminal-symbolic"
  if (mime.includes("json") || mime.includes("xml") || mime.includes("toml") || mime.includes("yaml"))
    return "text-x-generic-symbolic"
  if (mime.includes("markdown")) return "text-x-generic-symbolic"
  if (mime.startsWith("text/")) return "text-x-generic-symbolic"
  return "text-x-generic-symbolic"
}

function actionIcon(action: string): string {
  switch (action) {
    case "created": return "document-new-symbolic"
    case "modified": return "document-edit-symbolic"
    case "deleted": return "edit-delete-symbolic"
    case "opened":
    default: return "document-open-symbolic"
  }
}

// Pure imperative GTK widget construction — no JSX, no tracking context needed
function buildFileEntry(file: HotFile): Gtk.Widget {
  const sourceClassMap: Record<Source, string> = {
    claude: "source-claude",
    codex: "source-codex",
    system: "source-system",
    user: "source-user",
  }
  const sourceClass = sourceClassMap[file.source]

  const entryBox = new Gtk.Box()
  entryBox.cssClasses = ["file-entry", sourceClass]

  // Main file button
  const fileButton = new Gtk.Button()
  fileButton.cssClasses = ["file-button"]
  fileButton.hexpand = true

  // Gesture for click + shift-click
  const gesture = new Gtk.GestureClick()
  gesture.set_button(1)
  gesture.connect("released", () => {
    try {
      const state = gesture.get_current_event_state()
      const shift = (state & Gdk.ModifierType.SHIFT_MASK) !== 0
      if (shift) {
        const dirUri = GLib.filename_to_uri(file.fullDir, null)
        Gio.AppInfo.launch_default_for_uri(dirUri, null)
      } else {
        const uri = GLib.filename_to_uri(file.path, null)
        Gio.AppInfo.launch_default_for_uri(uri, null)
      }
    } catch (e) {
      console.error(`Failed to open ${file.path}:`, e)
    }
  })
  fileButton.add_controller(gesture)

  // Content layout
  const contentBox = new Gtk.Box()

  const icon = new Gtk.Image()
  icon.iconName = getIconForMime(file.mimeType)
  icon.cssClasses = ["file-icon"]
  contentBox.append(icon)

  const nameBox = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL })
  nameBox.hexpand = true

  const nameLabel = new Gtk.Label({ label: file.filename })
  nameLabel.cssClasses = ["file-name"]
  nameLabel.halign = Gtk.Align.START
  nameLabel.ellipsize = Pango.EllipsizeMode.END
  nameBox.append(nameLabel)

  const dirLabel = new Gtk.Label({ label: file.dir })
  dirLabel.cssClasses = ["file-dir"]
  dirLabel.halign = Gtk.Align.START
  dirLabel.ellipsize = Pango.EllipsizeMode.END
  nameBox.append(dirLabel)

  contentBox.append(nameBox)

  // Right side: time + action/source
  const metaBox = new Gtk.Box({ orientation: Gtk.Orientation.VERTICAL })
  metaBox.halign = Gtk.Align.END
  metaBox.valign = Gtk.Align.CENTER

  const timeLabel = new Gtk.Label({ label: timeAgo(file.timestamp) })
  timeLabel.cssClasses = ["file-time"]
  timeLabel.halign = Gtk.Align.END
  metaBox.append(timeLabel)

  const actionRow = new Gtk.Box()
  actionRow.halign = Gtk.Align.END

  const actionImg = new Gtk.Image()
  actionImg.iconName = actionIcon(file.action)
  actionImg.cssClasses = ["action-icon"]
  actionRow.append(actionImg)

  const sourceBadge = new Gtk.Label({ label: file.source })
  sourceBadge.cssClasses = ["source-badge", sourceClass]
  actionRow.append(sourceBadge)

  metaBox.append(actionRow)
  contentBox.append(metaBox)

  fileButton.set_child(contentBox)
  entryBox.append(fileButton)

  // Copy button
  const copyButton = new Gtk.Button()
  copyButton.cssClasses = ["copy-button"]
  copyButton.tooltipText = "Copy path"
  const copyIcon = new Gtk.Image()
  copyIcon.iconName = "edit-copy-symbolic"
  copyButton.set_child(copyIcon)
  copyButton.connect("clicked", () => {
    const display = Gdk.Display.get_default()
    if (!display) return
    const clipboard = display.get_clipboard()
    clipboard.set(file.path)
  })
  entryBox.append(copyButton)

  return entryBox
}

function buildEmptyState(): Gtk.Widget {
  const box = new Gtk.Box()
  box.cssClasses = ["empty-state"]
  box.hexpand = true
  box.vexpand = true
  box.halign = Gtk.Align.CENTER
  box.valign = Gtk.Align.CENTER
  const label = new Gtk.Label({ label: "No recent files" })
  box.append(label)
  return box
}

export default function Panel({ gdkmonitor }: { gdkmonitor: Gdk.Monitor }) {
  const service = HotFilesService.get_default()
  const [visible, setVisible] = createState(panelVisible)

  // Plain variables for current filter state (not reactive — we update UI imperatively)
  let currentFilter: Filter = service.filter
  let currentActionFilter: ActionFilter = service.actionFilter

  // Widget refs for imperative updates
  const sourceButtons = new Map<Filter, Gtk.Button>()
  const actionButtons = new Map<ActionFilter, Gtk.Button>()
  let fileListBox: Gtk.Box | null = null

  // Subscribe to visibility toggle
  const updateVisible = (v: boolean) => setVisible(v)
  panelListeners.add(updateVisible)
  onCleanup(() => panelListeners.delete(updateVisible))

  // Imperative: clear and rebuild the file list (no JSX — avoids tracking context crash)
  const rebuildFileList = () => {
    if (!fileListBox) return

    // Remove all existing children
    let child = fileListBox.get_first_child()
    while (child) {
      const next = child.get_next_sibling()
      fileListBox.remove(child)
      child = next
    }

    const currentFiles = service.files
    if (currentFiles.length === 0) {
      fileListBox.append(buildEmptyState())
    } else {
      for (const file of currentFiles) {
        fileListBox.append(buildFileEntry(file))
      }
    }
  }

  // Imperative: update source filter button active states
  const updateSourceButtons = () => {
    for (const [f, btn] of sourceButtons) {
      btn.cssClasses = f === currentFilter ? ["filter-chip", "active"] : ["filter-chip"]
    }
  }

  // Imperative: update action filter button active states
  const updateActionButtons = () => {
    for (const [f, btn] of actionButtons) {
      btn.cssClasses = f === currentActionFilter ? ["action-chip", "active"] : ["action-chip"]
    }
  }

  // Subscribe to service data changes (file watcher / poll triggers)
  const unsub = service.subscribe(() => {
    rebuildFileList()
  })
  onCleanup(() => unsub())

  const handleFilter = (f: Filter) => {
    currentFilter = f
    service.setFilter(f)
    updateSourceButtons()
    rebuildFileList()
  }

  const handleActionFilter = (f: ActionFilter) => {
    currentActionFilter = f
    service.setActionFilter(f)
    updateActionButtons()
    rebuildFileList()
  }

  const sourceFilters: Filter[] = ["all", "user", "claude", "codex", "system"]
  const actionFilters: ActionFilter[] = ["all", "opened", "modified", "created", "deleted"]

  let win: Astal.Window
  const { TOP, RIGHT, BOTTOM } = Astal.WindowAnchor

  onCleanup(() => {
    win?.destroy()
  })

  return (
    <window
      $={(self) => {
        win = self
        self.set_decorated(false)

        // Escape key dismisses the panel
        const keyCtl = new Gtk.EventControllerKey()
        keyCtl.connect("key-pressed", (_ctl: Gtk.EventControllerKey, keyval: number) => {
          if (keyval === Gdk.KEY_Escape) {
            togglePanel()
            return true
          }
          return false
        })
        self.add_controller(keyCtl)
      }}
      visible={visible}
      cssClasses={["hotbar-panel"]}
      namespace="hotbar"
      name={`hotbar-${gdkmonitor.connector}`}
      gdkmonitor={gdkmonitor}
      exclusivity={Astal.Exclusivity.NORMAL}
      keymode={Astal.Keymode.ON_DEMAND}
      anchor={TOP | RIGHT | BOTTOM}
      marginTop={8}
      marginRight={8}
      marginBottom={8}
      application={app}
    >
      <box orientation={1} cssClasses={["panel-content"]}>
        <box cssClasses={["panel-header"]} orientation={1}>
          <box>
            <label label="Hotbar" cssClasses={["panel-title"]} hexpand halign={1} />
          </box>
          <box cssClasses={["filter-row"]}>
            {sourceFilters.map((f) => (
              <button
                cssClasses={f === currentFilter ? ["filter-chip", "active"] : ["filter-chip"]}
                onClicked={() => handleFilter(f)}
                $={(self: Gtk.Button) => { sourceButtons.set(f, self) }}
              >
                <label label={f[0].toUpperCase() + f.slice(1)} />
              </button>
            ))}
          </box>
          <box cssClasses={["filter-row", "action-filter-row"]}>
            {actionFilters.map((f) => (
              <button
                cssClasses={f === currentActionFilter ? ["action-chip", "active"] : ["action-chip"]}
                onClicked={() => handleActionFilter(f)}
                $={(self: Gtk.Button) => { actionButtons.set(f, self) }}
              >
                <label label={f[0].toUpperCase() + f.slice(1)} />
              </button>
            ))}
          </box>
        </box>
        <Gtk.ScrolledWindow cssClasses={["panel-scroll"]} vexpand>
          <box
            orientation={1}
            cssClasses={["file-list"]}
            $={(self: Gtk.Box) => {
              fileListBox = self
              rebuildFileList()
            }}
          />
        </Gtk.ScrolledWindow>
      </box>
    </window>
  )
}
