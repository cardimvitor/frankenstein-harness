---
name: vue-2-legacy
scope: stack
stack: vue
versions: 2
keywords: vue vue2 options api vuex mixins filters vue-cli webpack data computed methods watch nexttick this
summary: Vue 2 (end of life) projects: stay in the Options API and existing patterns, avoid Vue 3-only syntax, make small safe changes.
---
- This is Vue 2 (end of life since 2023). Use the Options API and the patterns already in the project (`data`, `computed`, `methods`, `watch`, Vuex, mixins). Do not use `<script setup>`, `defineProps`, Teleport or other Vue 3-only features unless the project has the compat build or `@vue/composition-api`.
- Reactivity caveat: adding or removing object keys and setting array items by index needs `Vue.set` / `this.$set` / `splice`.
- `data` must be a function in components; keep `v-for` keys stable.
- Prefer minimal, local changes; do not migrate to Vue 3 as a side effect of another task.
