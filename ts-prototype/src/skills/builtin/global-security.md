---
name: security-senior
scope: global
keywords: security auth authorization injection xss csrf secret token password crypto session cors upload deserialization
summary: Senior security practice: validate input, parameterize, least privilege, no secrets in code.
---
- Check authorization on the server for every object access, not just authentication. Deny by default.
- Prevent injection: parameterized queries, no shell string concatenation, escape output for its context (HTML, attribute, URL, JS).
- Secrets come from configuration or a secret store, never source, logs, or client bundles. Rotate anything that leaked.
- Hash passwords with a slow adaptive hash; use vetted crypto libraries and current algorithms; compare secrets in constant time.
- Set cookies HttpOnly, Secure, SameSite; protect state-changing requests against CSRF; restrict CORS to known origins.
- Validate file uploads (type, size, path); never deserialize untrusted data with unsafe formatters.
- Do not weaken existing security controls to make a test pass.
