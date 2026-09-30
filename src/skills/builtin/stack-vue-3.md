---
name: vue-3
scope: stack
stack: vue
versions: 3
keywords: vue vue3 composition api script setup ref reactive computed watch pinia vue-router vite props emits defineprops defineemits defineModel slots composables nuxt
summary: Vue 3 conventions: script setup, Composition API, typed props/emits, composables, Pinia, keys in v-for.
---
- Use `<script setup>` with the Composition API. Declare props and emits with `defineProps` / `defineEmits` (typed in TypeScript projects); use `defineModel` for two-way bindings where the installed Vue supports it (3.4+).
- Keep state in `ref` / `reactive` and derived values in `computed`; do not mutate props. Side effects belong in `watch` / `watchEffect` with cleanup, or in lifecycle hooks; avoid watching what a computed can express.
- Give every `v-for` a stable `:key` (never the index for reorderable lists) and never combine `v-if` with `v-for` on one element.
- Extract reusable stateful logic into composables (`useXyz`) that return refs and functions; share app state with Pinia stores, not global mutable modules.
- Templates stay declarative: no heavy expressions, no direct DOM access (use template refs when unavoidable).
- Reactive props destructuring and `useTemplateRef` exist only in recent 3.5.x versions: check the installed version before using them.
- Test components through their rendered output and emitted events (Vue Test Utils or Testing Library), not implementation details.
