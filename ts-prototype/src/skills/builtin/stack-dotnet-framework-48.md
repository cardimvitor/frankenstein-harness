---
name: dotnet-framework-48
scope: stack
stack: dotnet-framework
keywords: dotnet framework csproj webforms mvc5 webapi owin iis web.config nuget packages.config msbuild wcf asmx entity framework 4.8
summary: .NET Framework 4.8 conventions: classic csproj/MSBuild, web.config, packages.config, ASP.NET MVC5/WebAPI2, EF6.
---
- Target is .NET Framework (Windows, MSBuild). Do not use APIs or syntax that require modern .NET; check the project's LangVersion (default C# 7.3 for net48) before using newer language features.
- Old-style csproj lists files explicitly: when adding a source file, add a <Compile Include> entry unless the project is SDK-style.
- Packages: check whether the project uses packages.config or PackageReference and keep to it; do not migrate as a side effect.
- Configuration lives in web.config/app.config; transforms per environment. Do not hard-code connection strings.
- ASP.NET MVC5 / Web API 2 / WebForms: follow the existing pattern in the folder you are editing. Async/await needs care with synchronization context (avoid .Result/.Wait()).
- Entity Framework 6: DbContext per request, avoid lazy-loading N+1 (Include), migrations via Code First if present.
- Build with MSBuild or `dotnet build` only if the SDK supports the project; tests via the project's runner (MSTest/NUnit/xUnit).
