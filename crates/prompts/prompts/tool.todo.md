The plan for this turn: one ordered checklist with stable ids (`t1`, `t2`, …). `action` is one of `read`, `set`, `add`, `update`, `remove` or `clear`.

- `read` returns the full list with ids and counts.
- `set` takes `items: [{content, status?}]`, replaces the list and assigns `t1…tN`.
- `add` takes `items` and an optional `after: id`; existing ids never change.
- `update` takes `updates: [{id, status?, content?}]` and patches them in one call.
- `remove` takes `ids`. `clear` empties the list.

Statuses are `pending`, `in_progress`, `completed` and `cancelled`; at most one item is `in_progress`. Keep the list current: mark an item `completed` as soon as its work is done.
