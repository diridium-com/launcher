// Copyright (c) Diridium Technologies Inc. All rights reserved.
// Licensed under the MPL-2.0 License. See LICENSE file in the project root.

import { ref } from "vue"
import { Channel, invoke } from "@tauri-apps/api/core"
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow"
import { save as saveDialog } from "@tauri-apps/plugin-dialog"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"

export interface ConsoleMsg {
  kind: "line" | "exit" | "reset"
  stream?: "out" | "err"
  text?: string
  status?: string
}

// Cap retained lines so a long-running, chatty engine can't grow webview memory
// and the DOM without bound. Mirrors the backend's backlog cap.
const MAX_LINES = 10000

export function useConsole() {
  const lines = ref<ConsoleMsg[]>([])
  const exited = ref<string | null>(null)
  // Monotonic counter of messages received. Consumers watch this for "new
  // output" instead of lines.length, which stops changing once the cap is hit.
  const received = ref(0)

  // Subscribe via a Channel. The backend replays its backlog through the same
  // channel before attaching it as the live sink, so backlog and live output
  // arrive in order with no gaps or duplicates — no client-side reordering.
  async function start() {
    const label = getCurrentWebviewWindow().label
    const channel = new Channel<ConsoleMsg>()
    channel.onmessage = (msg) => {
      if (msg.kind === "reset") exited.value = null
      else if (msg.kind === "exit") exited.value = msg.status ?? "process exited"
      lines.value.push(msg)
      if (lines.value.length > MAX_LINES) {
        lines.value.splice(0, lines.value.length - MAX_LINES)
      }
      received.value++
    }
    try {
      await invoke("console_subscribe", { label, on_line: channel })
    } catch (e) {
      lines.value.push({ kind: "line", stream: "err", text: `console error: ${e}` })
    }
  }

  function asText(): string {
    return lines.value
      .map((l) => (l.kind === "exit" ? `[${l.status}]` : l.text ?? ""))
      .join("\n")
  }

  function clear() {
    lines.value = []
  }

  async function copy() {
    await writeText(asText())
  }

  async function save() {
    const path = await saveDialog({
      defaultPath: "console.log",
      filters: [{ name: "Log", extensions: ["log", "txt"] }],
    })
    if (!path) return
    await invoke("console_save", { path, content: asText() })
  }

  return { lines, exited, received, start, clear, copy, save }
}
