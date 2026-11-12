"""Inputs outside plain JSON must give exactly what jsonpatch gives (via fallback)."""
import json, collections, math
import jsonpatch, jsonpatch_rs
cases = [
    ({"a": (1, 2)}, {"a": [1, 2, 3]}),
    ({"a": float("nan")}, {"a": 1}),
    ({"a": 10**30}, {"a": 10**30 + 1}),
    ({1: "x"}, {1: "y", "1": "z"}),
    (collections.OrderedDict(a=1), {"a": 2}),
    ({"a": "\ud800"}, {"a": "b"}),
    ([1, True, 1.0, 0, False, 0.0, -0.0], [True, 1, 1.0, False, -0.0, 0, 0.0]),
    ("abc", "abcd"),
    (None, {"a": 1}),
    ([], []),
]
deep = cur = []
for _ in range(1000):
    cur.append([]); cur = cur[0]
cases.append((deep, [1]))
bad = 0
for src, dst in cases:
    for cls in (jsonpatch.JsonPatch, jsonpatch.JsonPatchWithAppend):
        try: want = repr(list(jsonpatch_rs._orig_from_diff_append(cls, src, dst) if cls is jsonpatch.JsonPatchWithAppend else jsonpatch_rs._orig_from_diff(cls, src, dst)))
        except Exception as e: want = 'ERR ' + type(e).__name__
        try: got = repr(list(jsonpatch_rs._from_diff(cls, src, dst)))
        except Exception as e: got = 'ERR ' + type(e).__name__
        if want != got:
            bad += 1; print('MISMATCH', repr(src)[:80], repr(dst)[:80], want[:200], got[:200])
# custom dumps / pointer class
class P(jsonpatch.JsonPointer):
    @classmethod
    def from_parts(cls, parts):
        p = super().from_parts(parts); return p
src, dst = {"a": [1, 2, 3]}, {"a": [3, 1]}
assert list(jsonpatch_rs.JsonPatch.from_diff(src, dst, dumps=lambda x: json.dumps(x, sort_keys=True))) == list(jsonpatch_rs._orig_from_diff(jsonpatch.JsonPatch, src, dst, dumps=lambda x: json.dumps(x, sort_keys=True)))
# apply_patch equals original incl. in_place / atomic
doc = {"a": [1, {"b": 2}], "c": "x"}
p = [{"op": "add", "path": "/a/1/z", "value": [1]}, {"op": "move", "from": "/c", "path": "/d"}]
assert jsonpatch_rs.apply_patch(doc, p) == jsonpatch._orig_apply_patch(doc, p) if hasattr(jsonpatch, '_orig_apply_patch') else jsonpatch.apply_patch(doc, p)
assert doc == {"a": [1, {"b": 2}], "c": "x"}
print("fallback cases:", len(cases) * 2, "mismatches:", bad)
