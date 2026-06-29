<script setup lang="ts">
import { ref, nextTick, watch, onMounted } from "vue"

const { lines, exited, received, start, clear, copy, save } = useConsole()
const { theme: consoleTheme, toggle: toggleConsoleTheme } = useConsoleTheme()

const scrollEl = ref<HTMLElement | null>(null)
const stickToBottom = ref(true)

function onScroll() {
  const el = scrollEl.value
  if (!el) return
  // Keep auto-scrolling only while the user is near the bottom.
  stickToBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 24
}

watch(
  () => received.value,
  async () => {
    if (!stickToBottom.value) return
    await nextTick()
    const el = scrollEl.value
    if (el) el.scrollTop = el.scrollHeight
  },
)

onMounted(start)
</script>

<template>
  <div class="flex flex-col h-screen bg-surface-0 text-text-primary">
    <header class="flex items-center gap-2 px-3 py-2 border-b border-border bg-surface-1 select-none">
      <icon name="ph:terminal-window" class="text-text-secondary" />
      <span class="text-sm font-medium">Console</span>
      <span v-if="exited" class="text-xs text-text-tertiary italic">— {{ exited }}</span>
      <div class="ml-auto flex items-center gap-1">
        <button
          class="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 transition-colors hover:cursor-pointer"
          :title="consoleTheme === 'dark' ? 'Switch to light' : 'Switch to dark'"
          @click="toggleConsoleTheme"
        >
          <icon :name="consoleTheme === 'dark' ? 'ph:sun' : 'ph:moon'" />
        </button>
        <span class="w-px h-4 bg-border mx-1" />
        <button
          class="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 transition-colors hover:cursor-pointer"
          title="Clear"
          @click="clear"
        >
          <icon name="ph:trash" />
        </button>
        <button
          class="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 transition-colors hover:cursor-pointer"
          title="Copy"
          @click="copy"
        >
          <icon name="ph:copy" />
        </button>
        <button
          class="p-1.5 rounded text-text-secondary hover:text-text-primary hover:bg-surface-2 transition-colors hover:cursor-pointer"
          title="Save"
          @click="save"
        >
          <icon name="ph:floppy-disk" />
        </button>
      </div>
    </header>

    <div
      ref="scrollEl"
      class="log-pane flex-1 overflow-auto px-3 py-2 font-mono text-xs leading-relaxed"
      @scroll="onScroll"
    >
      <div
        v-for="(l, i) in lines"
        :key="i"
        class="whitespace-pre-wrap break-words"
        :class="
          l.kind === 'exit' || l.kind === 'reset'
            ? 'text-text-tertiary italic'
            : l.stream === 'err'
              ? 'text-danger'
              : 'text-console-text'
        "
      >
        <template v-if="l.kind === 'exit'">[{{ l.status }}]</template>
        <template v-else>{{ l.text }}</template>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* The global stylesheet disables text selection except on inputs; re-enable it
   here so console output can be selected and copied manually. */
.log-pane {
  -webkit-user-select: text;
  user-select: text;
}
</style>
