<script setup lang="ts">
import { ref } from "vue"
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow"

useZoom()
const { init: initTheme } = useTheme()
const { init: initConsoleTheme } = useConsoleTheme()

// Console windows load the same SPA as the main window; switch the view based
// on the window label (set by the Rust WebviewWindowBuilder as "console-<id>").
const isConsoleWindow = ref(false)
if (import.meta.client) {
  try {
    isConsoleWindow.value = getCurrentWebviewWindow().label.startsWith("console-")
  } catch {
    // not running inside a Tauri webview; treat as the main window
  }
}

onMounted(() => {
  // The console window owns its theme independently of the main window.
  if (isConsoleWindow.value) initConsoleTheme()
  else initTheme()
  document.addEventListener("contextmenu", (e) => e.preventDefault())
})
</script>

<template>
  <console-log v-if="isConsoleWindow" />
  <nuxt-page v-else class="h-screen overflow-hidden" />
</template>
