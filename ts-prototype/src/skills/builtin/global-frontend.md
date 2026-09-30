---
name: frontend-senior
scope: global
keywords: ui component state form accessibility responsive css layout event render loading empty error template
summary: Senior frontend practice: small components, explicit states, accessibility, no logic in templates.
---
- Model every screen with explicit states: loading, empty, error, success. Never leave a blank screen on failure.
- Keep components small and single-purpose; lift state only as far as needed; derive values instead of duplicating state.
- Forms: label every input, show validation next to the field, disable double-submit, keep user input on error.
- Accessibility is required: semantic elements, keyboard operability, visible focus, alt text, sufficient contrast, aria only when semantics are not enough.
- Never build HTML from untrusted strings; use the framework's binding and escaping.
- Match the project's existing component library, naming and styling approach before adding anything new.
- Avoid layout shifts and heavy work in render paths; memoize only after measuring.
