---
params: [matches = list<path = str>, query = str]
---

Results for "{{ query }}":

> {% for match in matches %}

- {{ match.path }}

> {% /for %}
