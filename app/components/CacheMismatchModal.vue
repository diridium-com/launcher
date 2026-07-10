<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from "vue"

defineProps<{
  engineType: string
  version: string
  jars: string[]
}>()

const emit = defineEmits<{ confirm: []; cancel: [] }>()

// Cancel is the safe default action; Escape dismisses.
const cancelBtn = ref<HTMLButtonElement | null>(null)
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") emit("cancel")
}
onMounted(() => {
  cancelBtn.value?.focus()
  window.addEventListener("keydown", onKey)
})
onBeforeUnmount(() => window.removeEventListener("keydown", onKey))
</script>

<template>
  <Teleport to="body">
    <Transition name="fade" appear>
      <div class="fixed inset-0 z-[100] flex items-center justify-center">
        <div class="absolute inset-0 bg-black/40 backdrop-blur-sm" @click="emit('cancel')" />
        <div
          class="relative bg-surface-1 border border-border rounded-xl shadow-overlay w-full max-w-md mx-4 p-6 space-y-5"
        >
          <header class="flex items-start gap-3">
            <div class="flex items-center justify-center size-10 rounded-full shrink-0 bg-danger/15">
              <icon name="ph:warning-octagon" class="text-lg text-danger" />
            </div>
            <div class="min-w-0">
              <h2 class="text-base font-semibold text-danger">Cached files don't match this server</h2>
              <p class="text-sm text-text-secondary mt-0.5">
                The cache for engine type "{{ engineType }}", version {{ version }}, holds files whose
                contents differ from what this server sent.
              </p>
            </div>
          </header>

          <div class="space-y-3 text-sm">
            <p class="text-text-secondary">
              The same engine version always ships the same files. A mismatch usually means another
              connection has this engine type but points at a different engine, so they share one cache.
              Continue to re-download the files for this connection, or cancel to fix the engine type.
            </p>
            <div>
              <p class="text-xs uppercase tracking-wider text-text-tertiary">
                Differing files ({{ jars.length }})
              </p>
              <ul
                class="font-mono text-xs bg-surface-2 rounded-md px-3 py-2 text-text-secondary max-h-40 overflow-y-auto space-y-0.5 leading-relaxed"
              >
                <li v-for="j in jars" :key="j" class="break-all">{{ j }}</li>
              </ul>
            </div>
          </div>

          <footer class="flex justify-end gap-2 pt-1">
            <!-- Cancel is the safe, prominent action; overwriting the cache is
                 the secondary, weightier choice. -->
            <button
              ref="cancelBtn"
              class="px-3 py-1.5 rounded-md text-sm bg-accent text-white hover:bg-accent-hover hover:cursor-pointer transition-colors"
              @click="emit('cancel')"
            >
              Cancel
            </button>
            <button
              class="px-3 py-1.5 rounded-md text-sm border border-danger text-danger hover:bg-danger/10 hover:cursor-pointer transition-colors"
              @click="emit('confirm')"
            >
              Continue anyway
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
