Apply a hashline patch sent as `input`. The header carries `[path#TAG]`; ops are `PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:` and `CUT N`/`CUT N.=M`; body lines start with `+`. Anchors must come from the view the tag names.

Use for existing files, even multi-file changes. Copy the tag and line numbers from a current view; never invent them. Replace line 4 like this:

```
[path#TAG]
PUT 4.=4:
+replacement line
```

`PUT 4:` is invalid. After a syntax error, correct the patch; after a stale-view error, read the range again. Supported bash reads can also supply tagged views; bash is not inherently incompatible with edit.
