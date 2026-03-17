import { createBinding, For, This } from "ags"
import app from "ags/gtk4/app"
import GLib from "gi://GLib"
import Gio from "gi://Gio"
import Gtk from "gi://Gtk?version=4.0"
import style from "./styles/style.scss"
import Panel, { togglePanel } from "./widgets/Panel"
import { setDebugConfig } from "./services/hotfiles"

GLib.set_prgname("hotbar")

// Parse command line flags
const args = ARGV || []
const debugEnabled = args.includes("--debug") || args.includes("-d")
const debugFileArg = args.find(a => a.startsWith("--file=") || a.startsWith("-f="))
const debugFile = debugFileArg ? debugFileArg.split("=")[1] :
                  (args.includes("--file") || args.includes("-f")) ?
                  `${GLib.get_home_dir()}/.cache/hotbar/debug.log` : null

// Ensure log directory exists if file logging enabled
if (debugFile) {
  const logDir = debugFile.substring(0, debugFile.lastIndexOf("/"))
  GLib.mkdir_with_parents(logDir, 0o755)
}

setDebugConfig(debugEnabled, debugFile)

// Force Adwaita icons — Yaru/Cosmic themes cause GTK4 infinite recursion crash
const settings = Gtk.Settings.get_default()
if (settings) {
  settings.gtk_icon_theme_name = "Adwaita"
  Object.defineProperty(settings, "gtk_icon_theme_name", {
    value: "Adwaita",
    writable: false,
    configurable: false,
  })
}

app.start({
  instanceName: "hotbar",
  css: style,
  requestHandler(request, respond) {
    const cmd = String(request)
    if (cmd === "toggle") {
      togglePanel()
      respond("toggled")
    } else if (cmd === "quit") {
      app.quit()
      respond("bye")
    } else {
      respond(`unknown: ${cmd}`)
    }
  },
  main() {
    const monitors = createBinding(app, "monitors")

    return (
      <For each={monitors}>
        {(monitor) => (
          <This this={app}>
            <Panel gdkmonitor={monitor} />
          </This>
        )}
      </For>
    )
  },
})
