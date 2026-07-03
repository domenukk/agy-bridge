---
allow_unused: true
types:
  - FileMatch = struct(path = str)

params:
  - matches = list(FileMatch)
  - query = str
---

Results for "{{ query }}":

> {% for match in matches %}

- {{ match.path }}

> {% /for %}
