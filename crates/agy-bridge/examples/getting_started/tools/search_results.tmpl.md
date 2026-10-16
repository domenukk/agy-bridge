---
allow_unused: true
types:
  - FileMatch = struct(path = untrusted str)

params:
  - matches = list(FileMatch)
  - query = untrusted str
---

Results for "{{ query }}":

> {% for match in matches %}

- {{ match.path }}

> {% /for %}
