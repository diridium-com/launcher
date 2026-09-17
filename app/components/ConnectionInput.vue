<script setup lang="ts">
const model = defineModel()

defineProps<{
  label?: string
  type: string
  placeholder?: string
  /// Short annotation shown alongside the label rather than on its own line,
  /// so it costs no height. For anything longer than a few words, put it in
  /// the page instead.
  note?: string
}>()

const inputId = useId()
</script>

<template>
  <div class="space-y-1">
    <div v-if="label" class="flex items-baseline justify-between gap-2">
      <label
        :for="inputId"
        class="block text-sm font-medium text-text-secondary select-none"
      >
        {{ label }}
      </label>
      <span v-if="note" class="text-xs text-text-tertiary select-none shrink-0">{{ note }}</span>
    </div>
    <input
      :id="inputId"
      :type="type"
      :placeholder="placeholder"
      class="w-full bg-surface-1 border border-border rounded-md px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-disabled outline-none transition-colors duration-100 focus:border-border-focus focus:ring-1 focus:ring-accent/30"
      v-model="model"
    />
  </div>
</template>
