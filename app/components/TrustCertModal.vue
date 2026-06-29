<script setup lang="ts">
import { computed, ref, onMounted, onBeforeUnmount } from "vue"
import type { CertInfo } from "~/types"

const props = defineProps<{
  mode: "first-use" | "changed"
  cert: CertInfo
  previousSha256?: string
}>()

const emit = defineEmits<{ confirm: []; cancel: [] }>()

const isChanged = computed(() => props.mode === "changed")

// Make the safe action (Cancel) the keyboard default, and let Escape dismiss.
const cancelBtn = ref<HTMLButtonElement | null>(null)
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") emit("cancel")
}
onMounted(() => {
  cancelBtn.value?.focus()
  window.addEventListener("keydown", onKey)
})
onBeforeUnmount(() => window.removeEventListener("keydown", onKey))

// Format like `openssl x509 -fingerprint -sha256` so it can be compared
// against what the server operator reads out.
function fmt(hex: string): string {
  return (hex.toUpperCase().match(/.{2}/g) || []).join(":")
}
const fingerprint = computed(() => fmt(props.cert.sha256))
const previousFingerprint = computed(() => (props.previousSha256 ? fmt(props.previousSha256) : ""))
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
            <div
              class="flex items-center justify-center size-10 rounded-full shrink-0"
              :class="isChanged ? 'bg-danger/15' : 'bg-accent/15'"
            >
              <icon
                :name="isChanged ? 'ph:warning-octagon' : 'ph:shield-check'"
                class="text-lg"
                :class="isChanged ? 'text-danger' : 'text-accent'"
              />
            </div>
            <div class="min-w-0">
              <h2
                class="text-base font-semibold"
                :class="isChanged ? 'text-danger' : 'text-text-primary'"
              >
                {{ isChanged ? "This server's certificate changed" : "Trust this server's certificate?" }}
              </h2>
              <p class="text-sm text-text-secondary mt-0.5">
                {{
                  isChanged
                    ? "It does not match the certificate you previously trusted. If you did not expect this, do not continue."
                    : "First time connecting to this server. Verify the fingerprint with the operator before trusting."
                }}
              </p>
            </div>
          </header>

          <div class="space-y-3 text-sm">
            <div v-if="cert.subject">
              <p class="text-xs uppercase tracking-wider text-text-tertiary">Subject</p>
              <p class="text-text-primary break-all">{{ cert.subject }}</p>
            </div>
            <div v-if="cert.issuer">
              <p class="text-xs uppercase tracking-wider text-text-tertiary">Issued by</p>
              <p class="text-text-primary break-all">{{ cert.issuer }}</p>
            </div>
            <div v-if="cert.not_after">
              <p class="text-xs uppercase tracking-wider text-text-tertiary">Expires</p>
              <p class="text-text-primary">{{ cert.not_after }}</p>
            </div>

            <div v-if="isChanged && previousFingerprint">
              <p class="text-xs uppercase tracking-wider text-text-tertiary">Previously trusted</p>
              <p
                class="font-mono text-xs bg-surface-2 rounded-md px-3 py-2 text-text-tertiary break-all leading-relaxed"
              >
                {{ previousFingerprint }}
              </p>
            </div>
            <div>
              <p
                class="text-xs uppercase tracking-wider"
                :class="isChanged ? 'text-danger' : 'text-text-tertiary'"
              >
                {{ isChanged ? "New SHA-256 fingerprint" : "SHA-256 fingerprint" }}
              </p>
              <p
                class="font-mono text-xs rounded-md px-3 py-2 break-all leading-relaxed"
                :class="
                  isChanged
                    ? 'bg-danger/10 text-text-primary border border-danger/30'
                    : 'bg-surface-2 text-text-secondary'
                "
              >
                {{ fingerprint }}
              </p>
            </div>
          </div>

          <footer class="flex justify-end gap-2 pt-1">
            <!-- In the cert-changed case the safe action is prominent; trusting
                 a new cert is the secondary, weightier choice. -->
            <button
              ref="cancelBtn"
              class="px-3 py-1.5 rounded-md text-sm hover:cursor-pointer transition-colors"
              :class="isChanged ? 'bg-accent text-white hover:bg-accent-hover' : 'text-text-secondary hover:bg-surface-2'"
              @click="emit('cancel')"
            >
              Cancel
            </button>
            <button
              class="px-3 py-1.5 rounded-md text-sm hover:cursor-pointer transition-colors"
              :class="isChanged ? 'border border-danger text-danger hover:bg-danger/10' : 'bg-accent text-white hover:bg-accent-hover'"
              @click="emit('confirm')"
            >
              {{ isChanged ? "Trust new certificate" : "Trust certificate" }}
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
