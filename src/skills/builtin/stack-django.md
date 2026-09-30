---
name: django
scope: stack
stack: django
keywords: django python orm queryset model migration migrations view serializer drf rest framework admin forms template settings middleware celery signals select_related prefetch_related manage.py test
summary: Django conventions: fat models/thin views, queryset efficiency, migrations for every model change, settings via environment, TestCase-based tests.
---
- Every model change needs a migration (`makemigrations`), reviewed and committed with the change; never edit an applied migration. Data migrations use `RunPython` with a reverse function.
- Avoid N+1 queries: use `select_related` for foreign keys and `prefetch_related` for reverse/many-to-many; filter and aggregate in the database, not in Python loops.
- Keep views thin: validation in forms or serializers, business rules in model methods or service functions, queries in managers/querysets.
- Wrap multi-step writes in `transaction.atomic`; use `F()` expressions and `update()` for counters instead of read-modify-write.
- Settings come from the environment (secrets, DEBUG, allowed hosts); never commit secrets and never run production with `DEBUG=True`. Use the ORM and parameterized queries, never string-built SQL.
- Version-specific APIs: `GeneratedField` and `db_default` need Django 5.0+, composite primary keys need 5.2+; new Django releases raise the minimum Python version. Check the installed version first.
- Tests use `django.test.TestCase` (or pytest-django if the project already does); create data with factories or fixtures, assert on behavior, and run the suite with `python manage.py test` unless the project configures something else.
