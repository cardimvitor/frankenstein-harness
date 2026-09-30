---
name: react-18-19
scope: stack
stack: react
versions: 18,19
keywords: react hooks useeffect usestate context component jsx tsx props memo suspense rsc next vite testing library
summary: React 18/19 conventions: function components, hooks rules, effects only for synchronization, keys, controlled inputs.
---
- Function components and hooks. Follow the Rules of Hooks (top level, same order every render).
- useEffect is for synchronizing with external systems, not for deriving state; compute derived values during render. Include all dependencies; clean up subscriptions and timers.
- Give list items stable keys (ids, not array index for reorderable lists).
- Do not mutate state or props; use immutable updates. Prefer controlled inputs.
- Lift state only as far as needed; use context sparingly for truly shared values.
- React 19: `use`, actions and `useActionState`/`useFormStatus` exist; ref is a regular prop for function components. React 18: concurrent features, automatic batching. Check the project's installed version before using a 19-only API.
- Keep components typed (props interfaces in TypeScript projects). Test behavior with Testing Library, not implementation.
