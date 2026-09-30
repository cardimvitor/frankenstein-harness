---
name: hig-ui-baseline
scope: global
keywords: ui ux design interface screen layout typography spacing color dark mode accessibility button navigation animation component style
summary: Default UI baseline inspired by Apple Human Interface Guidelines (used only when the project has no design system of its own).
---
Use this only when the project has no design system or clear UI pattern, or the user asks for it. If the project has its own, follow the project.
- Clarity: one primary action per screen; clear hierarchy through size, weight and spacing, not decoration.
- Deference: content first; chrome is quiet. Prefer system fonts and native controls.
- Consistency: reuse the same component for the same job; predictable navigation and placement.
- Spacing on a 4/8 px grid; comfortable touch/click targets (about 44px minimum); generous margins.
- Typography: a system font stack, a small fixed type scale, body text at 16px or larger, line length under about 75 characters.
- Color: semantic tokens (background, surface, text, accent, danger). Support light and dark mode. Text contrast at least 4.5:1; never rely on color alone.
- Feedback: immediate response to every action, visible focus, clear loading and error states, undo for destructive actions.
- Motion: short (150-300 ms), purposeful, and disabled under prefers-reduced-motion.
- Accessibility: keyboard reachable, screen-reader labels, resizable text.
