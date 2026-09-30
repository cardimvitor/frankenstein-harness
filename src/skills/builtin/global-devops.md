---
name: devops-senior
scope: global
keywords: docker dockerfile ci pipeline deploy kubernetes yaml build cache environment config release github actions
summary: Senior DevOps practice: reproducible builds, small images, config from environment, safe pipelines.
---
- Builds must be reproducible: pin versions, commit lockfiles, no network fetches of unpinned code at deploy time.
- Docker: multi-stage builds, run as non-root, order layers so dependency install is cached, keep secrets out of images and build args.
- Configuration comes from the environment; keep one artifact across environments.
- CI: fail fast, cache dependencies, run the same commands developers run locally, least-privilege tokens.
- Deployments should be repeatable and roll-back-able; add health checks and readable logs.
- Do not disable failing pipeline steps to get green.
