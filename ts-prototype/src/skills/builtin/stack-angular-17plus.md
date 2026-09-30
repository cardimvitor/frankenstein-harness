---
name: angular-17plus
scope: stack
stack: angular
versions: 17+
keywords: angular component signal standalone rxjs observable service injectable module template control flow ngif ngfor httpclient forms
summary: Angular 17+ conventions: standalone components, signals, built-in control flow, inject(), RxJS hygiene.
---
- Prefer standalone components (no NgModule) in new code; follow whichever style the project already uses.
- Angular 17+ templates support built-in control flow (@if, @for with track, @switch); use it if the project's version supports it, else *ngIf/*ngFor.
- Signals (signal, computed, effect) for local reactive state; keep effects for side effects only. Use OnPush change detection where the project does.
- Use inject() or constructor injection consistently with the codebase. Services hold logic; components stay presentational.
- RxJS: unsubscribe (async pipe, takeUntilDestroyed); avoid nested subscribes; use switchMap/concatMap deliberately.
- Forms: typed reactive forms; validate in the form model.
- Check the project's Angular major version before using newer APIs.
