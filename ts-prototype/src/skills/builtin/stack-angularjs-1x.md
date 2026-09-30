---
name: angularjs-1x
scope: stack
stack: angularjs
keywords: angularjs angular 1 controller scope directive ng-app ng-controller digest $http $scope factory service module ng-repeat
summary: AngularJS 1.x conventions: modules/controllers/directives, controllerAs, digest cycle, no ES module assumptions.
---
- AngularJS 1.x (end-of-life): make minimal, safe changes and match the existing style. Do not introduce Angular 2+ syntax or ES modules unless the build already supports them.
- Use controllerAs and avoid $scope inheritance pitfalls (bind to an object property, not a primitive). Minification-safe DI: array annotation or ng-annotate as the project does.
- Changes outside Angular (setTimeout, native events) need $scope.$apply/$timeout to update the view.
- Prefer components/directives with isolated scope; avoid heavy watchers and deep watches in ng-repeat.
- $http returns promises ($q); handle errors; do not build HTML from user data without $sanitize/ng-bind.
