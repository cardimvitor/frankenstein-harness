---
name: spring-boot
scope: stack
stack: spring-boot
keywords: spring boot java kotlin controller service repository jpa hibernate autowired configuration properties transactional validation restcontroller webmvc webflux actuator maven gradle testcontainers jakarta
summary: Spring Boot 3/4 conventions: constructor injection, layered structure, validation, ProblemDetail errors, slice tests.
---
- Constructor injection only (no field `@Autowired`); keep controllers thin, business rules in services, persistence in repositories. Do not return JPA entities from controllers: use DTOs or records.
- Bind settings with `@ConfigurationProperties` (validated), not scattered `@Value`. Put `@Transactional` on service methods, keep transactions short, and watch for lazy-loading outside a transaction.
- Validate input with Bean Validation (`@Valid`, `@NotBlank`, ...) and map errors to one consistent shape, `ProblemDetail` (RFC 9457) via `@RestControllerAdvice`.
- Spring Boot 3 and later use the `jakarta.*` namespace (not `javax.*`) and need Java 17+. Spring Boot 4 (Spring Framework 7) splits auto-configuration into modular starters and moves JSON to Jackson 3 (`tools.jackson` packages): check the installed Boot version and the existing imports before writing code.
- Prefer slice tests (`@WebMvcTest`, `@DataJpaTest`) and Testcontainers for real databases over mocking the persistence layer; keep `@SpringBootTest` for a few end-to-end checks.
- Virtual threads (`spring.threads.virtual.enabled`) are available from Boot 3.2 on Java 21; do not enable them as a drive-by change.
