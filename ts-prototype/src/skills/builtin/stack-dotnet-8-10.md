---
name: dotnet-8-10
scope: stack
stack: dotnet
versions: 8,9,10
keywords: dotnet csharp aspnet core minimal api ef core dependency injection appsettings nullable async records linq blazor net8 net9 net10
summary: Modern .NET 8/10 (LTS) conventions: SDK-style projects, DI, EF Core, minimal APIs, nullable reference types.
---
- SDK-style projects: files are globbed automatically; do not list Compile items. Check TargetFramework and LangVersion before using newer C# features.
- Use built-in dependency injection; register services in Program.cs; prefer constructor injection; avoid service locator.
- Async all the way with CancellationToken on I/O; never block with .Result/.Wait().
- Nullable reference types: respect annotations, do not silence with ! without a reason.
- EF Core: DbContext is scoped; use AsNoTracking for read-only queries; avoid N+1 with Include or projection; migrations via `dotnet ef`.
- Configuration through IConfiguration/Options pattern; secrets via user-secrets or environment, not appsettings in the repo.
- ASP.NET Core: validate models, return ProblemDetails for errors, keep endpoints thin.
- Test with the project's framework (xUnit/NUnit/MSTest) via `dotnet test`.
