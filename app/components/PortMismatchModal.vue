<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from "vue"

defineProps<{
  configuredPort: number
  advertisedPort: number
  advertisedUrl: string
}>()

// confirm carries the checkbox, so the caller can persist the suppression
// without a second round trip.
const emit = defineEmits<{ confirm: [suppress: boolean]; cancel: [] }>()

const suppress = ref(false)

// Continue is the likely action here, unlike the cache-mismatch prompt: the
// launch usually still works, and the operator mainly needs to know why the
// administrator is about to point somewhere else. Escape still cancels.
const continueBtn = ref<HTMLButtonElement | null>(null)
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") emit("cancel")
}
onMounted(() => {
  continueBtn.value?.focus()
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
            <div class="flex items-center justify-center size-10 rounded-full shrink-0 bg-accent/15">
              <icon name="ph:info" class="text-lg text-accent" />
            </div>
            <div class="min-w-0">
              <h2 class="text-base font-semibold text-text-primary">The server advertises a different port</h2>
              <p class="text-sm text-text-secondary mt-0.5">
                The server's JNLP says the administrator should connect on port
                <span class="font-mono text-text-primary">{{ advertisedPort }}</span
                >, but you configured port
                <span class="font-mono text-text-primary">{{ configuredPort }}</span
                >. The administrator may not be able to reach the server.
              </p>
            </div>
          </header>

          <div class="space-y-3 text-sm">
            <div>
              <p class="text-xs uppercase tracking-wider text-text-tertiary">Address the administrator will use</p>
              <!-- Readonly input, not a <p>: main.css disables user-select on
                   body and exempts only inputs, and this is the value worth
                   pasting into a browser to see whether it answers. -->
              <input
                type="text"
                readonly
                spellcheck="false"
                :value="advertisedUrl"
                :title="advertisedUrl"
                class="w-full min-w-0 font-mono text-xs bg-surface-2 rounded-md px-3 py-2 text-text-secondary outline-none cursor-text"
                @focus="($event.target as HTMLInputElement).select()"
              />
            </div>
            <p class="text-text-tertiary text-xs leading-relaxed">
              Only the port is compared. Hostnames differ routinely through a tunnel or port forward, so
              those are ignored.
            </p>
          </div>

          <label
            class="flex items-center gap-2 text-sm text-text-secondary hover:cursor-pointer select-none"
          >
            <input type="checkbox" class="accent-accent" v-model="suppress" />
            Don't show this again for this connection
          </label>

          <footer class="flex justify-end gap-2 pt-1">
            <button
              class="px-3 py-1.5 rounded-md text-sm border border-border text-text-secondary hover:bg-surface-2 hover:cursor-pointer transition-colors"
              @click="emit('cancel')"
            >
              Cancel
            </button>
            <button
              ref="continueBtn"
              class="px-3 py-1.5 rounded-md text-sm bg-accent text-white hover:bg-accent-hover hover:cursor-pointer transition-colors"
              @click="emit('confirm', suppress)"
            >
              Continue
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
